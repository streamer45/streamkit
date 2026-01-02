// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Peer Node - bidirectional server that accepts WebTransport connections
//!
//! This node supports a publish/subscribe architecture:
//! - One publisher connects to `{gateway_path}/input` to send audio
//! - Multiple subscribers connect to `{gateway_path}/output` to receive processed audio

use async_trait::async_trait;
use bytes::Buf;
use moq_lite::coding::Decode;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};
use tokio::sync::{broadcast, mpsc, OwnedSemaphorePermit, Semaphore};

/// Capacity for the broadcast channel (subscribers)
const SUBSCRIBER_BROADCAST_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
struct NodeStatsDelta {
    received: u64,
    sent: u64,
    discarded: u64,
    errored: u64,
}

#[derive(Clone, Debug)]
struct BroadcastFrame {
    data: bytes::Bytes,
    duration_us: Option<u64>,
}

/// Result of processing a single frame
enum FrameResult {
    /// Continue processing more frames
    Continue,
    /// Current group is exhausted, need to get next group
    GroupExhausted,
    /// Shutdown was requested or output closed
    Shutdown,
}

#[derive(Debug)]
enum PublisherEvent {
    Connected { path: String },
    Disconnected { path: String, error: Option<String> },
}

struct BidirectionalTaskConfig {
    input_broadcast: String,
    output_broadcast: String,
    output_sender: streamkit_core::OutputSender,
    broadcast_rx: broadcast::Receiver<BroadcastFrame>,
    shutdown_rx: broadcast::Receiver<()>,
    publisher_slot: Arc<Semaphore>,
    publisher_events: mpsc::UnboundedSender<PublisherEvent>,
    subscriber_count: Arc<AtomicU64>,
    output_group_duration_ms: u64,
    output_initial_delay_ms: u64,
    stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
}

struct PublisherReceiveLoopWithSlotConfig {
    subscribe: moq_lite::OriginConsumer,
    broadcast_name: String,
    output_sender: streamkit_core::OutputSender,
    publisher_slot: Arc<Semaphore>,
    publisher_events: mpsc::UnboundedSender<PublisherEvent>,
    publisher_path: String,
    stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
}

fn normalize_gateway_path(path: &str) -> String {
    let trimmed = path.trim();
    let trimmed = if trimmed.is_empty() { "/moq" } else { trimmed };
    let without_trailing = trimmed.trim_end_matches('/');
    let normalized = if without_trailing.is_empty() { "/" } else { without_trailing };
    if normalized == "/" || normalized.starts_with('/') {
        normalized.to_string()
    } else {
        format!("/{normalized}")
    }
}

fn join_gateway_path(base: &str, suffix: &str) -> String {
    if base == "/" {
        format!("/{suffix}")
    } else {
        format!("{base}/{suffix}")
    }
}

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct MoqPeerConfig {
    /// Broadcast name to receive from publisher client
    pub input_broadcast: String,
    /// Broadcast name to send to subscriber clients
    pub output_broadcast: String,
    /// Base path for gateway routing (e.g., "/moq")
    /// Publishers connect to "{gateway_path}/input", subscribers to "{gateway_path}/output"
    pub gateway_path: String,
    /// Allow publisher reconnections without recreating the session
    pub allow_reconnect: bool,
    /// Duration of each MoQ group in milliseconds for the subscriber output.
    ///
    /// Default: 40ms (2 Opus frames at 20ms each).
    pub output_group_duration_ms: u64,
    /// Adds a timestamp offset (playout delay) so receivers can buffer before playback.
    ///
    /// Default: 0 (no added delay).
    pub output_initial_delay_ms: u64,
}

impl Default for MoqPeerConfig {
    fn default() -> Self {
        Self {
            input_broadcast: "input".to_string(),
            output_broadcast: "output".to_string(),
            gateway_path: "/moq".to_string(),
            allow_reconnect: false,
            output_group_duration_ms: 40,
            output_initial_delay_ms: 0,
        }
    }
}

/// A MoQ server node that supports one publisher and multiple subscribers.
/// - Publisher connects to `{gateway_path}/input` and sends audio to the pipeline
/// - Subscribers connect to `{gateway_path}/output` and receive processed audio
pub struct MoqPeerNode {
    config: MoqPeerConfig,
}

impl MoqPeerNode {
    pub const fn new(config: MoqPeerConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ProcessorNode for MoqPeerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::OpusAudio],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::OpusAudio,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let gateway_path = normalize_gateway_path(&self.config.gateway_path);
        let base_path = gateway_path.clone();
        let input_path = join_gateway_path(&gateway_path, "input");
        let output_path = join_gateway_path(&gateway_path, "output");

        tracing::info!(
            gateway_path = %gateway_path,
            base_path = %base_path,
            input_path = %input_path,
            output_path = %output_path,
            input_broadcast = %self.config.input_broadcast,
            output_broadcast = %self.config.output_broadcast,
            allow_reconnect = %self.config.allow_reconnect,
            output_group_duration_ms = self.config.output_group_duration_ms,
            output_initial_delay_ms = self.config.output_initial_delay_ms,
            session_id = ?context.session_id,
            "MoqPeerNode starting with separate input/output paths"
        );

        // Get session ID (required for gateway registration)
        let session_id = context.session_id.as_ref().ok_or_else(|| {
            let err = "moq_peer requires a session_id for gateway registration";
            tracing::error!("{}", err);
            StreamKitError::Configuration(err.to_string())
        })?;

        // Get gateway from global registry
        let gateway = streamkit_core::moq_gateway::get_moq_gateway().ok_or_else(|| {
            let err =
                "MoQ gateway not available - ensure moq_peer is used in a session with gateway support";
            tracing::error!("{}", err);
            StreamKitError::Runtime(err.to_string())
        })?;

        // Register both paths with gateway
        tracing::info!(
            input_path = %input_path,
            output_path = %output_path,
            session_id = %session_id,
            "Registering MoQ routes with gateway"
        );

        let mut base_connection_rx =
            gateway.register_route(base_path.clone(), session_id.clone()).await.map_err(|e| {
                let err = format!("Failed to register base gateway route: {e}");
                tracing::error!("{}", err);
                StreamKitError::Runtime(err)
            })?;

        let mut input_connection_rx =
            gateway.register_route(input_path.clone(), session_id.clone()).await.map_err(|e| {
                let err = format!("Failed to register input gateway route: {e}");
                tracing::error!("{}", err);
                StreamKitError::Runtime(err)
            })?;

        let mut output_connection_rx =
            gateway.register_route(output_path.clone(), session_id.clone()).await.map_err(|e| {
                let err = format!("Failed to register output gateway route: {e}");
                tracing::error!("{}", err);
                StreamKitError::Runtime(err)
            })?;

        // Take ownership of pipeline input channel
        let mut pipeline_input_rx = context.take_input("in")?;

        // Create broadcast channel for fanning out to subscribers
        let (subscriber_broadcast_tx, _) =
            broadcast::channel::<BroadcastFrame>(SUBSCRIBER_BROADCAST_CAPACITY);

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let (stats_delta_tx, mut stats_delta_rx) = mpsc::channel::<NodeStatsDelta>(1024);

        // Subscriber count for logging
        let subscriber_count = Arc::new(AtomicU64::new(0));

        // Shutdown signal
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        // Track publisher connection state
        let publisher_slot = Arc::new(Semaphore::new(1));
        let (publisher_events_tx, mut publisher_events_rx) =
            mpsc::unbounded_channel::<PublisherEvent>();

        state_helpers::emit_running(&context.state_tx, &node_name);
        tracing::info!(
            "MoqPeerNode ready - connect clients at: {} (or {} / {})",
            gateway_path,
            input_path,
            output_path
        );

        let final_result = loop {
            tokio::select! {
                // Accept bidirectional peer connections on the base path
                Some(conn) = base_connection_rx.recv() => {
                    tracing::info!(path = %conn.path, "Peer connecting");

                    let sub_count = subscriber_count.clone();
                    let broadcast_rx = subscriber_broadcast_tx.subscribe();

                    match Self::start_bidirectional_task(
                        conn,
                        BidirectionalTaskConfig {
                            input_broadcast: self.config.input_broadcast.clone(),
                            output_broadcast: self.config.output_broadcast.clone(),
                            output_sender: context.output_sender.clone(),
                            broadcast_rx,
                            shutdown_rx: shutdown_tx.subscribe(),
                            publisher_slot: publisher_slot.clone(),
                            publisher_events: publisher_events_tx.clone(),
                            subscriber_count: sub_count,
                            output_group_duration_ms: self.config.output_group_duration_ms,
                            output_initial_delay_ms: self.config.output_initial_delay_ms,
                            stats_delta_tx: stats_delta_tx.clone(),
                        },
                    ).await {
                        Ok(_handle) => {
                            let count = subscriber_count.fetch_add(1, Ordering::SeqCst) + 1;
                            tracing::info!("Peer connected (total: {})", count);
                        }
                        Err(e) => {
                            tracing::error!("Failed to start peer task: {}", e);
                        }
                    }
                }

                // Accept publisher connections on /input path
                Some(conn) = input_connection_rx.recv() => {
                    let Ok(permit) = publisher_slot.clone().try_acquire_owned() else {
                        tracing::warn!(path = %conn.path, "Rejecting publisher connection - already have a publisher");
                        let _ = conn.response_tx.send(
                            streamkit_core::moq_gateway::MoqConnectionResult::Rejected(
                                "Publisher already connected".to_string()
                            )
                        );
                        continue;
                    };

                    tracing::info!(path = %conn.path, "Publisher connecting");

                    match Self::start_publisher_task_with_permit(
                        conn,
                        permit,
                        self.config.input_broadcast.clone(),
                        context.output_sender.clone(),
                        shutdown_tx.subscribe(),
                        publisher_events_tx.clone(),
                        stats_delta_tx.clone(),
                    ).await {
                        Ok(_handle) => {
                            tracing::info!("Publisher connected and streaming");
                        }
                        Err(e) => {
                            tracing::error!("Failed to start publisher task: {}", e);
                        }
                    }
                }

                // Accept subscriber connections on /output path
                Some(conn) = output_connection_rx.recv() => {
                    tracing::info!(path = %conn.path, "Subscriber connecting");

                    let sub_count = subscriber_count.clone();
                    let broadcast_rx = subscriber_broadcast_tx.subscribe();

                    match Self::start_subscriber_task(
                        conn,
                        self.config.output_broadcast.clone(),
                        broadcast_rx,
                        shutdown_tx.subscribe(),
                        sub_count,
                        self.config.output_group_duration_ms,
                        self.config.output_initial_delay_ms,
                        stats_delta_tx.clone(),
                    ).await {
                        Ok(_handle) => {
                            let count = subscriber_count.fetch_add(1, Ordering::SeqCst) + 1;
                            tracing::info!("Subscriber connected (total: {})", count);
                        }
                        Err(e) => {
                            tracing::error!("Failed to start subscriber task: {}", e);
                        }
                    }
                }

                // Forward packets from pipeline to broadcast channel
                packet = pipeline_input_rx.recv() => {
                    if let Some(packet) = packet {
                        if let Packet::Binary { data, metadata, .. } = packet {
                            stats_tracker.received();
                            // Broadcast to all subscribers (ignore if no receivers)
                            let duration_us = super::constants::packet_duration_us(metadata.as_ref());
                            let _ = subscriber_broadcast_tx.send(BroadcastFrame { data, duration_us });
                            stats_tracker.sent();
                            stats_tracker.maybe_send();
                        }
                    } else {
                        tracing::info!("Pipeline input closed");
                        break Ok(());
                    }
                }

                Some(delta) = stats_delta_rx.recv() => {
                    if delta.received > 0 {
                        stats_tracker.received_n(delta.received);
                    }
                    if delta.sent > 0 {
                        stats_tracker.sent_n(delta.sent);
                    }
                    if delta.discarded > 0 {
                        stats_tracker.discarded_n(delta.discarded);
                    }
                    if delta.errored > 0 {
                        stats_tracker.errored_n(delta.errored);
                    }
                    stats_tracker.maybe_send();
                }

                // Publisher lifecycle events (from both /input and base-path peers)
                Some(event) = publisher_events_rx.recv() => {
                    match event {
                        PublisherEvent::Connected { path } => {
                            tracing::info!(path = %path, "Publisher connected");
                            state_helpers::emit_running(&context.state_tx, &node_name);
                        }
                        PublisherEvent::Disconnected { path, error } => {
                            if let Some(err) = error {
                                tracing::warn!(path = %path, error = %err, "Publisher disconnected with error");
                            } else {
                                tracing::info!(path = %path, "Publisher disconnected");
                            }

                            if !self.config.allow_reconnect {
                                tracing::info!("Publisher reconnection disabled, shutting down");
                                break Ok(());
                            }

                            tracing::info!("Waiting for publisher reconnection...");
                            state_helpers::emit_recovering(
                                &context.state_tx,
                                &node_name,
                                "waiting_for_publisher",
                                None,
                            );
                        }
                    }
                }

                // Check for shutdown signal
                Some(control_msg) = context.control_rx.recv() => {
                    match control_msg {
                        streamkit_core::control::NodeControlMessage::Shutdown => {
                            tracing::info!("Received shutdown signal");
                            break Ok(());
                        }
                        _ => {
                            tracing::debug!("Ignoring control message");
                        }
                    }
                }
            }
        };

        // Cleanup: signal all tasks to shutdown
        let _ = shutdown_tx.send(());

        // Unregister routes from gateway
        tracing::info!("Unregistering MoQ routes from gateway");
        gateway.unregister_route(&base_path).await;
        gateway.unregister_route(&input_path).await;
        gateway.unregister_route(&output_path).await;

        // Send final stats
        stats_tracker.force_send();

        state_helpers::emit_stopped(&context.state_tx, &node_name, "shutdown");
        tracing::info!(
            "MoqPeerNode finished with {} active subscribers",
            subscriber_count.load(Ordering::SeqCst)
        );
        final_result
    }
}

impl MoqPeerNode {
    /// Start a task to handle publisher connection (receives audio from client)
    async fn start_publisher_task_with_permit(
        moq_connection: streamkit_core::moq_gateway::MoqConnection,
        permit: OwnedSemaphorePermit,
        input_broadcast: String,
        output_sender: streamkit_core::OutputSender,
        mut shutdown_rx: broadcast::Receiver<()>,
        publisher_events: mpsc::UnboundedSender<PublisherEvent>,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    ) -> Result<tokio::task::JoinHandle<Result<(), StreamKitError>>, StreamKitError> {
        let path = moq_connection.path.clone();

        // Extract the WebTransport session
        let web_transport_session = *moq_connection
            .session
            .downcast::<moq_native::web_transport_quinn::Session>()
            .map_err(|_| {
                StreamKitError::Runtime("Invalid WebTransport session type".to_string())
            })?;

        // Notify gateway that we accepted the connection
        let _ = moq_connection
            .response_tx
            .send(streamkit_core::moq_gateway::MoqConnectionResult::Accepted);

        // Create origin for receiving from client
        let client_publish_origin = moq_lite::Origin::produce();
        let receive_origin = client_publish_origin.consumer.clone();

        // Accept MoQ session (publisher only sends, no server publish needed)
        let session = moq_lite::Session::accept(
            web_transport_session,
            None, // No server publish origin - publisher is receive-only
            Some(client_publish_origin.producer),
        )
        .await
        .map_err(|e| StreamKitError::Runtime(format!("Failed to accept session: {e}")))?;

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let _ = publisher_events.send(PublisherEvent::Connected { path: path.clone() });

            let result = Self::publisher_receive_loop(
                receive_origin,
                input_broadcast,
                output_sender,
                &mut shutdown_rx,
                stats_delta_tx,
            )
            .await;

            let _ = publisher_events.send(PublisherEvent::Disconnected {
                path,
                error: result.as_ref().err().map(std::string::ToString::to_string),
            });

            // Keep session alive until task ends
            drop(session);
            result
        });

        Ok(handle)
    }

    async fn start_bidirectional_task(
        moq_connection: streamkit_core::moq_gateway::MoqConnection,
        config: BidirectionalTaskConfig,
    ) -> Result<tokio::task::JoinHandle<()>, StreamKitError> {
        let path = moq_connection.path.clone();

        // Extract the WebTransport session
        let web_transport_session = *moq_connection
            .session
            .downcast::<moq_native::web_transport_quinn::Session>()
            .map_err(|_| {
                StreamKitError::Runtime("Invalid WebTransport session type".to_string())
            })?;

        // Notify gateway that we accepted the connection
        let _ = moq_connection
            .response_tx
            .send(streamkit_core::moq_gateway::MoqConnectionResult::Accepted);

        // Create origins for full bidirectional MoQ
        let server_publish_origin = moq_lite::Origin::produce();
        let send_origin = server_publish_origin.producer.clone();

        let client_publish_origin = moq_lite::Origin::produce();
        let receive_origin = client_publish_origin.consumer.clone();

        let session = moq_lite::Session::accept(
            web_transport_session,
            Some(server_publish_origin.consumer),
            Some(client_publish_origin.producer),
        )
        .await
        .map_err(|e| StreamKitError::Runtime(format!("Failed to accept session: {e}")))?;

        let handle = tokio::spawn(async move {
            let mut publisher_shutdown_rx = config.shutdown_rx.resubscribe();
            let mut subscriber_shutdown_rx = config.shutdown_rx;

            // Clone stats_delta_tx before async blocks to avoid borrow conflicts
            let publisher_stats_delta_tx = config.stats_delta_tx.clone();
            let subscriber_stats_delta_tx = config.stats_delta_tx;

            let publisher_fut = async {
                Self::publisher_receive_loop_with_slot(
                    PublisherReceiveLoopWithSlotConfig {
                        subscribe: receive_origin,
                        broadcast_name: config.input_broadcast,
                        output_sender: config.output_sender,
                        publisher_slot: config.publisher_slot,
                        publisher_events: config.publisher_events,
                        publisher_path: path.clone(),
                        stats_delta_tx: publisher_stats_delta_tx,
                    },
                    &mut publisher_shutdown_rx,
                )
                .await
            };

            let subscriber_fut = async {
                Self::subscriber_send_loop(
                    send_origin,
                    config.output_broadcast,
                    config.broadcast_rx,
                    &mut subscriber_shutdown_rx,
                    config.output_group_duration_ms,
                    config.output_initial_delay_ms,
                    subscriber_stats_delta_tx,
                )
                .await
            };

            let (publisher_result, subscriber_result) = tokio::join!(publisher_fut, subscriber_fut);

            if let Err(e) = publisher_result {
                tracing::warn!(path = %path, error = %e, "Peer publisher task error");
            }
            if let Err(e) = subscriber_result {
                tracing::warn!(path = %path, error = %e, "Peer subscriber task error");
            }

            let count = config.subscriber_count.fetch_sub(1, Ordering::SeqCst) - 1;
            tracing::info!(path = %path, "Peer disconnected (remaining: {})", count);

            drop(session);
        });

        Ok(handle)
    }

    async fn publisher_receive_loop_with_slot(
        config: PublisherReceiveLoopWithSlotConfig,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<(), StreamKitError> {
        tracing::info!(
            path = %config.publisher_path,
            "Waiting for peer publisher to announce broadcast: {}",
            config.broadcast_name
        );

        let Some(broadcast_consumer) = Self::wait_for_broadcast_announcement(
            config.subscribe,
            &config.broadcast_name,
            shutdown_rx,
        )
        .await?
        else {
            return Ok(());
        };

        let Ok(permit) = config.publisher_slot.try_acquire_owned() else {
            tracing::warn!(
                path = %config.publisher_path,
                "Ignoring peer publisher broadcast - publisher already connected"
            );
            return Ok(());
        };

        let _ = config
            .publisher_events
            .send(PublisherEvent::Connected { path: config.publisher_path.clone() });

        let result = async {
            let Some((audio_track_name, audio_priority)) =
                Self::wait_for_catalog_with_audio(&broadcast_consumer, shutdown_rx).await?
            else {
                return Ok(());
            };

            tracing::info!(
                path = %config.publisher_path,
                "Subscribing to peer publisher audio track: {}",
                audio_track_name
            );

            let track_consumer = broadcast_consumer.subscribe_track(&moq_lite::Track {
                name: audio_track_name,
                priority: audio_priority,
            });

            Self::process_publisher_frames(
                track_consumer,
                config.output_sender,
                shutdown_rx,
                &config.stats_delta_tx,
            )
            .await
        }
        .await;

        drop(permit);
        let _ = config.publisher_events.send(PublisherEvent::Disconnected {
            path: config.publisher_path,
            error: result.as_ref().err().map(std::string::ToString::to_string),
        });

        result
    }

    /// Publisher receive loop - receives audio from client and sends to pipeline
    async fn publisher_receive_loop(
        subscribe: moq_lite::OriginConsumer,
        broadcast_name: String,
        output_sender: streamkit_core::OutputSender,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    ) -> Result<(), StreamKitError> {
        tracing::info!("Waiting for publisher to announce broadcast: {}", broadcast_name);

        // Wait for client to announce the broadcast
        let Some(broadcast_consumer) =
            Self::wait_for_broadcast_announcement(subscribe, &broadcast_name, shutdown_rx).await?
        else {
            return Ok(()); // Shutdown requested
        };

        // Wait for catalog with audio track info
        let Some((audio_track_name, audio_priority)) =
            Self::wait_for_catalog_with_audio(&broadcast_consumer, shutdown_rx).await?
        else {
            return Ok(()); // Shutdown requested
        };

        tracing::info!("Subscribing to publisher audio track: {}", audio_track_name);

        let track_consumer = broadcast_consumer
            .subscribe_track(&moq_lite::Track { name: audio_track_name, priority: audio_priority });

        // Process incoming frames
        Self::process_publisher_frames(track_consumer, output_sender, shutdown_rx, &stats_delta_tx)
            .await
    }

    /// Wait for the publisher to announce the expected broadcast
    async fn wait_for_broadcast_announcement(
        mut subscribe: moq_lite::OriginConsumer,
        broadcast_name: &str,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<Option<moq_lite::BroadcastConsumer>, StreamKitError> {
        loop {
            tokio::select! {
                announcement = subscribe.announced() => {
                    match announcement {
                        Some((path, Some(consumer))) => {
                            tracing::info!("Publisher announced broadcast: {}", path.as_str());
                            if path.as_str() == broadcast_name {
                                return Ok(Some(consumer));
                            }
                        }
                        Some((path, None)) => {
                            tracing::info!("Publisher unannounced broadcast: {}", path.as_str());
                        }
                        None => {
                            return Err(StreamKitError::Runtime("Origin consumer closed".to_string()));
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    tracing::info!("Publisher task shutting down");
                    return Ok(None);
                }
            }
        }
    }

    /// Wait for the catalog to contain audio track information
    async fn wait_for_catalog_with_audio(
        broadcast_consumer: &moq_lite::BroadcastConsumer,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<Option<(String, u8)>, StreamKitError> {
        let catalog_track =
            broadcast_consumer.subscribe_track(&hang::catalog::Catalog::default_track());
        let mut catalog_consumer = hang::catalog::CatalogConsumer::new(catalog_track);

        loop {
            tokio::select! {
                catalog_result = tokio::time::timeout(Duration::from_secs(10), catalog_consumer.next()) => {
                    let catalog = catalog_result
                        .map_err(|_| StreamKitError::Runtime("Timeout waiting for catalog".to_string()))?
                        .map_err(|e| StreamKitError::Runtime(format!("Failed to read catalog: {e}")))?
                        .ok_or_else(|| StreamKitError::Runtime("Catalog track closed".to_string()))?;

                    tracing::info!("Received catalog from publisher: audio={:?}", catalog.audio);

                    if let Some(audio) = &catalog.audio {
                        if let Some(track_name) = audio.renditions.keys().next() {
                            tracing::info!("Found audio track in catalog: {}", track_name);
                            return Ok(Some((track_name.clone(), audio.priority)));
                        }
                    }
                    tracing::debug!("Catalog has no audio yet, waiting for update...");
                }
                _ = shutdown_rx.recv() => {
                    return Ok(None);
                }
            }
        }
    }

    /// Process incoming frames from the publisher and forward to the pipeline
    async fn process_publisher_frames(
        mut track_consumer: moq_lite::TrackConsumer,
        mut output_sender: streamkit_core::OutputSender,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
    ) -> Result<(), StreamKitError> {
        let mut frame_count = 0u64;
        let mut last_log = std::time::Instant::now();
        let mut current_group: Option<moq_lite::GroupConsumer> = None;

        loop {
            // Get a group if we don't have one
            if current_group.is_none() {
                match Self::get_next_group(&mut track_consumer, shutdown_rx).await? {
                    Some(group) => current_group = Some(group),
                    None => return Ok(()), // Stream ended or shutdown
                }
            }

            // Process frames from current group
            if let Some(ref mut group) = current_group {
                match Self::process_frame_from_group(
                    group,
                    &mut output_sender,
                    &mut frame_count,
                    &mut last_log,
                    shutdown_rx,
                    stats_delta_tx,
                )
                .await?
                {
                    FrameResult::Continue => {},
                    FrameResult::GroupExhausted => current_group = None,
                    FrameResult::Shutdown => return Ok(()),
                }
            }
        }
    }

    /// Get the next group from the track consumer
    async fn get_next_group(
        track_consumer: &mut moq_lite::TrackConsumer,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<Option<moq_lite::GroupConsumer>, StreamKitError> {
        tokio::select! {
            biased;
            group_result = track_consumer.next_group() => {
                match group_result {
                    Ok(Some(group)) => Ok(Some(group)),
                    Ok(None) => {
                        tracing::info!("Publisher stream ended");
                        Ok(None)
                    }
                    Err(e) => Err(StreamKitError::Runtime(format!("Error getting group: {e}"))),
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Publisher receive loop shutting down");
                Ok(None)
            }
        }
    }

    /// Process a single frame from the current group
    async fn process_frame_from_group(
        group: &mut moq_lite::GroupConsumer,
        output_sender: &mut streamkit_core::OutputSender,
        frame_count: &mut u64,
        last_log: &mut std::time::Instant,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
    ) -> Result<FrameResult, StreamKitError> {
        tokio::select! {
            biased;
            frame_result = group.read_frame() => {
                match frame_result {
                    Ok(Some(mut payload)) => {
                        *frame_count += 1;

                        if last_log.elapsed() > Duration::from_secs(1) {
                            tracing::debug!("Publisher: received {} frames/sec", *frame_count);
                            *frame_count = 0;
                            *last_log = std::time::Instant::now();
                        }

                        // Skip timestamp header (varint encoded u64 microseconds)
                        // The hang protocol encodes timestamp at the start of each frame
                        if let Err(e) = u64::decode(&mut payload, moq_lite::lite::Version::Draft02) {
                            tracing::warn!("Failed to decode frame timestamp: {e}");
                            let _ = stats_delta_tx
                                .try_send(NodeStatsDelta { received: 1, discarded: 1, ..Default::default() });
                            return Ok(FrameResult::Continue);
                        }

                        let data = payload.copy_to_bytes(payload.remaining());
                        let packet = Packet::Binary {
                            data,
                            content_type: None,
                            metadata: None,
                        };

                        if output_sender.send("out", packet).await.is_err() {
                            tracing::debug!("Output channel closed");
                            let _ = stats_delta_tx
                                .try_send(NodeStatsDelta { received: 1, ..Default::default() });
                            return Ok(FrameResult::Shutdown);
                        }
                        let _ = stats_delta_tx.try_send(NodeStatsDelta { received: 1, sent: 1, ..Default::default() });
                        Ok(FrameResult::Continue)
                    }
                    Ok(None) => Ok(FrameResult::GroupExhausted),
                    Err(e) => {
                        tracing::warn!("Error reading frame: {e}");
                        let _ = stats_delta_tx.try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                        Ok(FrameResult::GroupExhausted)
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!("Publisher receive loop shutting down");
                Ok(FrameResult::Shutdown)
            }
        }
    }

    /// Start a task to handle subscriber connection (sends audio to client)
    #[allow(clippy::too_many_arguments)]
    async fn start_subscriber_task(
        moq_connection: streamkit_core::moq_gateway::MoqConnection,
        output_broadcast: String,
        broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        mut shutdown_rx: broadcast::Receiver<()>,
        subscriber_count: Arc<AtomicU64>,
        output_group_duration_ms: u64,
        output_initial_delay_ms: u64,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    ) -> Result<tokio::task::JoinHandle<()>, StreamKitError> {
        // Extract the WebTransport session
        let web_transport_session = *moq_connection
            .session
            .downcast::<moq_native::web_transport_quinn::Session>()
            .map_err(|_| {
                StreamKitError::Runtime("Invalid WebTransport session type".to_string())
            })?;

        // Notify gateway that we accepted the connection
        let _ = moq_connection
            .response_tx
            .send(streamkit_core::moq_gateway::MoqConnectionResult::Accepted);

        // Create origin for sending to client
        let server_publish_origin = moq_lite::Origin::produce();
        let send_origin = server_publish_origin.producer.clone();

        // Accept MoQ session (subscriber only receives, no client publish needed)
        let session = moq_lite::Session::accept(
            web_transport_session,
            Some(server_publish_origin.consumer),
            None, // No client publish origin - subscriber is send-only
        )
        .await
        .map_err(|e| StreamKitError::Runtime(format!("Failed to accept session: {e}")))?;

        let handle = tokio::spawn(async move {
            let result = Self::subscriber_send_loop(
                send_origin,
                output_broadcast,
                broadcast_rx,
                &mut shutdown_rx,
                output_group_duration_ms,
                output_initial_delay_ms,
                stats_delta_tx,
            )
            .await;

            // Decrement subscriber count
            let count = subscriber_count.fetch_sub(1, Ordering::SeqCst) - 1;
            tracing::info!("Subscriber disconnected (remaining: {})", count);

            // Keep session alive until task ends
            drop(session);

            if let Err(e) = result {
                tracing::warn!("Subscriber task error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Subscriber send loop - receives from broadcast channel and sends to client
    async fn subscriber_send_loop(
        publish: moq_lite::OriginProducer,
        broadcast_name: String,
        broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        shutdown_rx: &mut broadcast::Receiver<()>,
        output_group_duration_ms: u64,
        output_initial_delay_ms: u64,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    ) -> Result<(), StreamKitError> {
        // Setup broadcast and tracks
        let (_broadcast_producer, mut track_producer, _catalog_producer) =
            Self::setup_subscriber_broadcast(&publish, &broadcast_name)?;

        tracing::info!("Published catalog to subscriber");

        // Run the send loop
        let packet_count = Self::run_subscriber_send_loop(
            &mut track_producer,
            broadcast_rx,
            shutdown_rx,
            output_group_duration_ms,
            output_initial_delay_ms,
            &stats_delta_tx,
        )
        .await?;

        track_producer.inner.clone().close();
        tracing::info!("Subscriber task finished after {} packets", packet_count);
        Ok(())
    }

    /// Setup broadcast, audio track, and catalog for subscriber
    fn setup_subscriber_broadcast(
        publish: &moq_lite::OriginProducer,
        broadcast_name: &str,
    ) -> Result<
        (moq_lite::BroadcastProducer, hang::TrackProducer, moq_lite::TrackProducer),
        StreamKitError,
    > {
        // Create broadcast
        let broadcast_produce = moq_lite::Broadcast::produce();
        publish.publish_broadcast(broadcast_name, broadcast_produce.consumer);
        let mut broadcast_producer = broadcast_produce.producer;

        // Create audio track
        let audio_track = moq_lite::Track { name: "audio/data".to_string(), priority: 80 };
        let track_producer = broadcast_producer.create_track(audio_track.clone());
        let track_producer: hang::TrackProducer = track_producer.into();

        // Create and publish catalog
        let catalog_producer =
            Self::create_and_publish_catalog(&mut broadcast_producer, &audio_track)?;

        Ok((broadcast_producer, track_producer, catalog_producer))
    }

    /// Create and publish the catalog with audio track info
    fn create_and_publish_catalog(
        broadcast_producer: &mut moq_lite::BroadcastProducer,
        audio_track: &moq_lite::Track,
    ) -> Result<moq_lite::TrackProducer, StreamKitError> {
        let mut audio_renditions = std::collections::BTreeMap::new();
        audio_renditions.insert(
            audio_track.name.clone(),
            hang::catalog::AudioConfig {
                codec: hang::catalog::AudioCodec::Opus,
                sample_rate: 48000,
                channel_count: 1,
                bitrate: Some(64_000),
                description: None,
            },
        );

        let catalog = hang::catalog::Catalog {
            audio: Some(hang::catalog::Audio { renditions: audio_renditions, priority: 80 }),
            ..Default::default()
        };

        let mut catalog_producer =
            broadcast_producer.create_track(hang::catalog::Catalog::default_track());
        let catalog_json = catalog
            .to_string()
            .map_err(|e| StreamKitError::Runtime(format!("Failed to serialize catalog: {e}")))?;
        catalog_producer.write_frame(catalog_json.into_bytes());

        Ok(catalog_producer)
    }

    /// Run the main send loop, forwarding packets to the subscriber
    async fn run_subscriber_send_loop(
        track_producer: &mut hang::TrackProducer,
        mut broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        shutdown_rx: &mut broadcast::Receiver<()>,
        output_group_duration_ms: u64,
        output_initial_delay_ms: u64,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
    ) -> Result<u64, StreamKitError> {
        let mut packet_count: u64 = 0;
        let mut last_log = std::time::Instant::now();
        let mut frame_count = 0u64;
        let group_duration_ms = output_group_duration_ms.max(1);
        let mut clock = super::constants::MediaClock::new(output_initial_delay_ms);

        loop {
            tokio::select! {
                recv_result = broadcast_rx.recv() => {
                    match Self::handle_broadcast_recv(
                        recv_result,
                        track_producer,
                        &mut packet_count,
                        &mut frame_count,
                        &mut last_log,
                        group_duration_ms,
                        &mut clock,
                        stats_delta_tx,
                    )? {
                        SendResult::Continue => {}
                        SendResult::Stop => break,
                    }
                }
                _ = shutdown_rx.recv() => {
                    tracing::info!("Subscriber send loop shutting down");
                    break;
                }
            }
        }

        Ok(packet_count)
    }

    /// Handle a single broadcast receive result
    #[allow(clippy::too_many_arguments)]
    fn handle_broadcast_recv(
        recv_result: Result<BroadcastFrame, broadcast::error::RecvError>,
        track_producer: &mut hang::TrackProducer,
        packet_count: &mut u64,
        frame_count: &mut u64,
        last_log: &mut std::time::Instant,
        group_duration_ms: u64,
        clock: &mut super::constants::MediaClock,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
    ) -> Result<SendResult, StreamKitError> {
        match recv_result {
            Ok(broadcast_frame) => {
                *packet_count += 1;
                *frame_count += 1;

                if last_log.elapsed() > Duration::from_secs(1) {
                    tracing::debug!("Subscriber: sent {} frames/sec", *frame_count);
                    *frame_count = 0;
                    *last_log = std::time::Instant::now();
                }

                let is_first = *packet_count == 1;
                let timestamp_ms = clock.timestamp_ms();
                let keyframe = is_first || clock.is_group_boundary(group_duration_ms);

                let timestamp = hang::Timestamp::from_millis(timestamp_ms).map_err(|_| {
                    StreamKitError::Runtime("MoQ frame timestamp overflow".to_string())
                })?;

                let mut payload = hang::BufList::new();
                payload.push_chunk(broadcast_frame.data);

                let frame = hang::Frame { timestamp, keyframe, payload };

                if let Err(e) = track_producer.write(frame) {
                    tracing::warn!("Failed to write MoQ frame to subscriber: {e}");
                    let _ = stats_delta_tx
                        .try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                    return Ok(SendResult::Stop);
                }
                clock.advance_by_duration_us(broadcast_frame.duration_us);
                Ok(SendResult::Continue)
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Subscriber lagged, dropped {} packets", n);
                let _ =
                    stats_delta_tx.try_send(NodeStatsDelta { discarded: n, ..Default::default() });
                Ok(SendResult::Continue)
            },
            Err(broadcast::error::RecvError::Closed) => {
                tracing::info!("Broadcast channel closed");
                Ok(SendResult::Stop)
            },
        }
    }
}

/// Result of sending a frame to a subscriber
enum SendResult {
    /// Continue sending more frames
    Continue,
    /// Stop the send loop
    Stop,
}
