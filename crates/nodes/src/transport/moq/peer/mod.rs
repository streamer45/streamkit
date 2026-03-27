// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Peer Node - bidirectional server that accepts WebTransport connections
//!
//! This node supports a publish/subscribe architecture:
//! - One publisher connects to `{gateway_path}/input` to send media
//! - Multiple subscribers connect to `{gateway_path}/output` to receive processed media
//!
//! Input and output pins are type-agnostic: both `in` and `in_1` accept any
//! supported encoded media type (Opus audio, VP9 video). The actual media kind
//! flowing through each pin is determined at runtime from `NodeContext::input_types`.

use crate::video::{VP9_BIT_DEPTH, VP9_LEVEL, VP9_PROFILE};
use async_trait::async_trait;
use bytes::Buf;
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketType, VideoCodec,
};
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};
use tokio::sync::{broadcast, mpsc, watch, OwnedSemaphorePermit, Semaphore};

/// Capacity for the broadcast channel (subscribers).
///
/// With audio+video multiplexed (~50 fps audio + 30 fps video = ~80 fps),
/// 256 entries give a slow subscriber roughly 3 seconds of buffer before
/// frames are dropped due to lagging. This is adequate for real-time
/// streaming; increase if subscribers are expected to be bursty-slow.
const SUBSCRIBER_BROADCAST_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Default)]
struct NodeStatsDelta {
    received: u64,
    sent: u64,
    discarded: u64,
    errored: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaKind {
    Audio,
    Video,
}

#[derive(Clone, Debug)]
struct BroadcastFrame {
    data: bytes::Bytes,
    duration_us: Option<u64>,
    kind: MediaKind,
    keyframe: bool,
}

/// Media type state shared from the main select loop to subscriber tasks via a
/// [`watch`] channel.  Subscribers wait for `resolved == true` before building
/// the MoQ catalog so that dynamic pipelines (where `input_types` is empty)
/// don't advertise an empty catalog.
#[derive(Clone, Debug)]
struct MediaTypeState {
    has_audio: bool,
    has_video: bool,
    /// `true` once the media kind of every connected input pin has been
    /// determined — either from `NodeContext::input_types` (static pipelines)
    /// or from the first packet on each pin (dynamic pipelines).
    resolved: bool,
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

/// Outcome of a publisher track processing loop.
///
/// Distinguishes a transient publisher-side cancellation (which the caller may
/// recover from by re-subscribing) from clean completion and fatal errors.
enum TrackExit {
    /// Track closed cleanly, stream ended, or shutdown was requested.
    /// The caller should not retry.
    Finished,
    /// The publisher cancelled the subscription (`moq_lite::Error::Cancel`).
    ///
    /// This typically happens when the browser's `@moq/hang` publish pipeline
    /// transiently tears down the track producer — e.g. when `camera.source`
    /// flaps to `undefined` during the permission-grant → device-enumeration
    /// cascade. The catalog still advertises the track and the browser's
    /// `Broadcast#runBroadcast` loop will happily accept a fresh subscription,
    /// so the caller may re-subscribe after a short backoff.
    Cancelled,
    /// A non-recoverable error occurred. Propagate up.
    Error(StreamKitError),
}

#[derive(Debug)]
enum PublisherEvent {
    Connected { path: String },
    Disconnected { path: String, error: Option<String> },
}

/// Media and output configuration shared across subscriber-related functions.
struct SubscriberMediaConfig {
    has_video: bool,
    has_audio: bool,
    video_width: u32,
    video_height: u32,
    output_group_duration_ms: u64,
    output_initial_delay_ms: u64,
}

struct BidirectionalTaskConfig {
    /// All input broadcast names — first is primary, rest are additional.
    input_broadcasts: Vec<String>,
    output_broadcast: String,
    node_id: String,
    output_sender: streamkit_core::OutputSender,
    broadcast_rx: broadcast::Receiver<BroadcastFrame>,
    shutdown_rx: broadcast::Receiver<()>,
    publisher_slot: Arc<Semaphore>,
    publisher_events: mpsc::UnboundedSender<PublisherEvent>,
    subscriber_count: Arc<AtomicU64>,
    stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    media: SubscriberMediaConfig,
    media_state_rx: watch::Receiver<MediaTypeState>,
    dynamic_outputs: DynamicOutputs,
}

struct PublisherReceiveLoopWithSlotConfig {
    subscribe: moq_lite::OriginConsumer,
    broadcast_name: String,
    output_sender: streamkit_core::OutputSender,
    publisher_slot: Arc<Semaphore>,
    publisher_events: mpsc::UnboundedSender<PublisherEvent>,
    publisher_path: String,
    stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    dynamic_outputs: DynamicOutputs,
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
    /// Broadcast names to receive from the publisher client.
    ///
    /// The first element is the primary broadcast (used for the dedicated
    /// `/input` sub-path).  Additional elements are only supported via
    /// bidirectional (base path) connections.  Output pins for tracks from
    /// non-primary broadcasts are namespaced as
    /// `{broadcast_name}/{track_name}` (e.g. `screen-input/video/hd`).
    pub input_broadcasts: Vec<String>,
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
    /// Video width in pixels for the MoQ catalog.
    /// Used to advertise the video resolution to subscribers.
    /// Default: 640.
    pub video_width: u32,
    /// Video height in pixels for the MoQ catalog.
    /// Used to advertise the video resolution to subscribers.
    /// Default: 480.
    pub video_height: u32,
}

impl Default for MoqPeerConfig {
    fn default() -> Self {
        Self {
            input_broadcasts: vec!["input".to_string()],
            output_broadcast: "output".to_string(),
            gateway_path: "/moq".to_string(),
            allow_reconnect: false,
            output_group_duration_ms: 40,
            output_initial_delay_ms: 0,
            video_width: 640,
            video_height: 480,
        }
    }
}

impl MoqPeerConfig {
    /// The primary (first) input broadcast name.
    ///
    /// Falls back to `"input"` if the vec is empty, but this should never
    /// happen in practice because [`MoqPeerNode::run`] validates that
    /// `input_broadcasts` is non-empty at startup.
    fn primary_input_broadcast(&self) -> &str {
        self.input_broadcasts.first().map_or("input", |s| s.as_str())
    }

    /// Additional input broadcast names beyond the primary.
    fn extra_input_broadcasts(&self) -> &[String] {
        if self.input_broadcasts.len() > 1 {
            &self.input_broadcasts[1..]
        } else {
            &[]
        }
    }
}

/// Infer the [`MediaKind`] from a [`PacketType`].
///
/// Returns `Some(Audio)` for encoded audio, `Some(Video)` for encoded video,
/// and `None` for anything else.
const fn media_kind_for_packet_type(pt: &PacketType) -> Option<MediaKind> {
    match pt {
        PacketType::EncodedAudio(_) => Some(MediaKind::Audio),
        PacketType::EncodedVideo(_) => Some(MediaKind::Video),
        _ => None,
    }
}

/// Infer [`MediaKind`] from a packet's `content_type` field.
///
/// VP9-encoded packets carry `content_type: Some("video/vp9")`, so any
/// content type starting with `"video/"` is classified as video.
/// Audio packets (Opus) typically have `content_type: None`.
///
/// **Important**: when `content_type` is `None` this defaults to audio.
/// Upstream nodes that produce video **must** set `content_type` (e.g.
/// `"video/vp9"`) for correct routing in dynamic pipelines. The
/// static-pipeline path uses `NodeContext::input_types` instead and is
/// unaffected.
///
/// # Panics (debug only)
///
/// Debug-asserts that the `content_type` is either `None` (audio) or starts
/// with `"audio/"` or `"video/"`.  This catches future encoders that forget
/// to set the field.
fn infer_kind_from_packet(packet: &Packet) -> MediaKind {
    if let Packet::Binary { content_type, .. } = packet {
        if let Some(ct) = content_type.as_deref() {
            if ct.starts_with("video/") {
                return MediaKind::Video;
            }
            debug_assert!(
                ct.starts_with("audio/"),
                "unexpected content_type {ct:?} — expected \"audio/…\" or \"video/…\""
            );
        } else {
            // No content_type → assume audio. This is correct for Opus packets
            // but would misclassify video packets that forgot to set the field.
            // Log at warn level on the first occurrence to aid debugging.
            use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, AtomicOrdering::Relaxed) {
                tracing::warn!(
                    "packet has no content_type — defaulting to audio; \
                     set content_type to \"video/…\" on video packets for correct routing"
                );
            }
        }
    }
    MediaKind::Audio
}

/// Build a [`BroadcastFrame`] from a pipeline [`Packet`] and the inferred
/// [`MediaKind`] for the pin it arrived on.
fn make_broadcast_frame(packet: Packet, kind: MediaKind) -> Option<BroadcastFrame> {
    if let Packet::Binary { data, metadata, .. } = packet {
        let duration_us = super::constants::packet_duration_us(metadata.as_ref());
        let keyframe = if kind == MediaKind::Video {
            // Default to true when keyframe metadata is missing so that the
            // subscriber's OrderedProducer opens an initial MoQ group on the
            // first frame. Matches the convention in push.rs.
            metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(true)
        } else {
            false
        };
        Some(BroadcastFrame { data, duration_us, kind, keyframe })
    } else {
        None
    }
}

/// Shared map of dynamically created output pin senders.
///
/// When downstream nodes connect to track-named pins (e.g. `moq_peer.audio/data`),
/// the engine creates the pin on-demand and sends the channel via
/// [`PinManagementMessage::AddedOutputPin`]. Track processors check this map
/// and forward frames to the corresponding downstream channel.
///
/// Uses [`std::sync::RwLock`] rather than [`tokio::sync::RwLock`] because the
/// lock is never held across an `.await` point — only brief synchronous reads
/// in [`route_packet`] and occasional writes when pins are added/removed.
/// This avoids the overhead of the async lock on every packet in the hot path.
type DynamicOutputs =
    Arc<std::sync::RwLock<std::collections::HashMap<String, mpsc::Sender<Packet>>>>;

/// A MoQ server node that supports one publisher and multiple subscribers.
/// - Publisher connects to `{gateway_path}/input` and sends media to the pipeline
/// - Subscribers connect to `{gateway_path}/output` and receive processed media
pub struct MoqPeerNode {
    config: MoqPeerConfig,
}

impl MoqPeerNode {
    pub const fn new(config: MoqPeerConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl ProcessorNode for MoqPeerNode {
    fn input_pins(&self) -> Vec<InputPin> {
        let accepted_types = super::constants::moq_accepted_media_types();
        vec![
            InputPin {
                name: "in".to_string(),
                accepts_types: accepted_types.clone(),
                cardinality: PinCardinality::One,
            },
            InputPin {
                name: "in_1".to_string(),
                accepts_types: accepted_types,
                cardinality: PinCardinality::One,
            },
        ]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![make_dynamic_output_pin("audio/data"), make_dynamic_output_pin("video/data")]
    }

    fn supports_dynamic_pins(&self) -> bool {
        true
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // Validate that at least one input broadcast is configured.
        if self.config.input_broadcasts.is_empty() {
            return Err(StreamKitError::Configuration(
                "input_broadcasts must contain at least one broadcast name".to_string(),
            ));
        }

        let gateway_path = normalize_gateway_path(&self.config.gateway_path);
        let base_path = gateway_path.clone();
        let input_path = join_gateway_path(&gateway_path, "input");
        let output_path = join_gateway_path(&gateway_path, "output");

        tracing::info!(
            gateway_path = %gateway_path,
            base_path = %base_path,
            input_path = %input_path,
            output_path = %output_path,
            input_broadcast = %self.config.primary_input_broadcast(),
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

        // Take ownership of pipeline input channels.
        // Both pins accept any encoded media type — the actual kind is
        // determined at runtime from `input_types`.
        let mut pin_0_rx = context.take_input("in").ok();
        let mut pin_1_rx = context.take_input("in_1").ok();

        // Try to get type info from the graph builder (static pipelines).
        // For dynamic pipelines `input_types` is empty — we handle that below.
        let mut pin_0_kind = context.input_types.get("in").and_then(media_kind_for_packet_type);
        let mut pin_1_kind = context.input_types.get("in_1").and_then(media_kind_for_packet_type);

        if pin_0_rx.is_none() && pin_1_rx.is_none() {
            return Err(StreamKitError::Configuration(
                "MoQ peer requires at least one input pin (\"in\" or \"in_1\")".to_string(),
            ));
        }

        let mut has_audio =
            pin_0_kind == Some(MediaKind::Audio) || pin_1_kind == Some(MediaKind::Audio);
        let mut has_video =
            pin_0_kind == Some(MediaKind::Video) || pin_1_kind == Some(MediaKind::Video);

        // NOTE: In dynamic pipelines the engine creates receivers for ALL
        // declared input pins, even those that are never wired to an upstream
        // node.  We therefore cannot use `pin_rx.is_some()` to decide
        // connectivity, and `input_types` is empty so pin kinds are unknown.
        //
        // We optimistically advertise both audio and video so that the MoQ
        // catalog is published immediately when a subscriber connects.  Tracks
        // that never receive data are harmless — the subscriber simply gets no
        // frames on them.  This avoids a race where the browser subscribes to
        // `catalog.json` before the catalog track has been created (which would
        // return "not found" and prevent the watch path from going live).
        let dynamic_mode =
            context.input_types.is_empty() && pin_0_kind.is_none() && pin_1_kind.is_none();
        if dynamic_mode {
            has_audio = true;
            has_video = true;
        }
        let types_resolved = if dynamic_mode {
            // Optimistically resolved — both types are advertised.
            true
        } else {
            let pin_0_connected = pin_0_rx.is_some();
            let pin_1_connected = pin_1_rx.is_some();
            (pin_0_kind.is_some() || !pin_0_connected) && (pin_1_kind.is_some() || !pin_1_connected)
        };

        let (media_state_tx, media_state_rx) =
            watch::channel(MediaTypeState { has_audio, has_video, resolved: types_resolved });

        // Warn if both pins carry the same media kind — this is likely a
        // misconfiguration since the node expects one audio and one video input.
        if let (Some(k0), Some(k1)) = (pin_0_kind, pin_1_kind) {
            if k0 == k1 {
                tracing::warn!(
                    kind = ?k0,
                    "Both input pins carry the same media kind — \
                     expected one audio and one video input for correct track multiplexing"
                );
            }
        }

        tracing::info!(
            has_audio,
            has_video,
            ?pin_0_kind,
            ?pin_1_kind,
            "MoQ peer input pins (types inferred at runtime)"
        );

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

        // Dynamic output pin channels — populated when the engine creates
        // track-named output pins (e.g. `audio/data`, `video/hd`) on demand.
        let dynamic_outputs: DynamicOutputs = Arc::default();

        // Pin management channel for runtime pin creation
        let mut pin_mgmt_rx = context.pin_management_rx.take();

        // Track JoinHandles for dynamic input forwarder tasks so they can be
        // aborted promptly when the corresponding pin is removed.
        let mut forwarder_handles: std::collections::HashMap<String, tokio::task::JoinHandle<()>> =
            std::collections::HashMap::new();

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
                    // Auth check: bidirectional needs both publish and subscribe permissions
                    if let Some(auth) = &conn.auth {
                        let input_bc = self.config.primary_input_broadcast();
                        let output_bc = &self.config.output_broadcast;

                        // Check auth for all input broadcasts (primary + additional)
                        let rejection_reason = if !auth.can_publish(input_bc) {
                            Some(format!("Publish permission denied for broadcast '{input_bc}'"))
                        } else if !auth.can_subscribe(output_bc) {
                            Some(format!("Subscribe permission denied for broadcast '{output_bc}'"))
                        } else {
                            self.config.extra_input_broadcasts().iter().find_map(|bc| {
                                if auth.can_publish(bc) {
                                    None
                                } else {
                                    Some(format!("Publish permission denied for additional broadcast '{bc}'"))
                                }
                            })
                        };

                        if let Some(reason) = rejection_reason {
                            tracing::warn!(
                                path = %conn.path,
                                input_broadcast = %input_bc,
                                output_broadcast = %output_bc,
                                "Rejecting bidirectional connection - {reason}"
                            );
                            let _ = conn.response_tx.send(
                                streamkit_core::moq_gateway::MoqConnectionResult::Rejected(reason)
                            );
                            continue;
                        }
                    }

                    tracing::info!(path = %conn.path, "Peer connecting");

                    let sub_count = subscriber_count.clone();
                    let broadcast_rx = subscriber_broadcast_tx.subscribe();

                    match Self::start_bidirectional_task(
                        conn,
                        BidirectionalTaskConfig {
                            input_broadcasts: self.config.input_broadcasts.clone(),
                            output_broadcast: self.config.output_broadcast.clone(),
                            node_id: node_name.clone(),
                            output_sender: context.output_sender.clone(),
                            broadcast_rx,
                            shutdown_rx: shutdown_tx.subscribe(),
                            publisher_slot: publisher_slot.clone(),
                            publisher_events: publisher_events_tx.clone(),
                            subscriber_count: sub_count,
                            stats_delta_tx: stats_delta_tx.clone(),
                            media: SubscriberMediaConfig {
                                has_video,
                                has_audio,
                                video_width: self.config.video_width,
                                video_height: self.config.video_height,
                                output_group_duration_ms: self.config.output_group_duration_ms,
                                output_initial_delay_ms: self.config.output_initial_delay_ms,
                            },
                            media_state_rx: media_state_rx.clone(),
                            dynamic_outputs: dynamic_outputs.clone(),
                        },
                    ).await {
                        Ok(_handle) => {
                            tracing::info!("Peer task started");
                        }
                        Err(e) => {
                            tracing::error!("Failed to start peer task: {}", e);
                        }
                    }
                }

                // Accept publisher connections on /input path
                Some(conn) = input_connection_rx.recv() => {
                    // Auth check: publisher needs publish permission
                    if let Some(auth) = &conn.auth {
                        let input_bc = self.config.primary_input_broadcast();

                        if !auth.can_publish(input_bc) {
                            tracing::warn!(
                                path = %conn.path,
                                broadcast = %input_bc,
                                "Rejecting publisher connection - publish permission denied"
                            );
                            let _ = conn.response_tx.send(
                                streamkit_core::moq_gateway::MoqConnectionResult::Rejected(
                                    format!("Publish permission denied for broadcast '{input_bc}'")
                                )
                            );
                            continue;
                        }
                    }

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
                        self.config.primary_input_broadcast().to_string(),
                        context.output_sender.clone(),
                        shutdown_tx.subscribe(),
                        publisher_events_tx.clone(),
                        stats_delta_tx.clone(),
                        dynamic_outputs.clone(),
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
                    // Auth check: subscriber needs subscribe permission
                    if let Some(auth) = &conn.auth {
                        let output_bc = &self.config.output_broadcast;

                        if !auth.can_subscribe(output_bc) {
                            tracing::warn!(
                                path = %conn.path,
                                broadcast = %output_bc,
                                "Rejecting subscriber connection - subscribe permission denied"
                            );
                            let _ = conn.response_tx.send(
                                streamkit_core::moq_gateway::MoqConnectionResult::Rejected(
                                    format!("Subscribe permission denied for broadcast '{output_bc}'")
                                )
                            );
                            continue;
                        }
                    }

                    tracing::info!(path = %conn.path, "Subscriber connecting");

                    let sub_count = subscriber_count.clone();
                    let broadcast_rx = subscriber_broadcast_tx.subscribe();

                    match Self::start_subscriber_task(
                        conn,
                        node_name.clone(),
                        self.config.output_broadcast.clone(),
                        broadcast_rx,
                        shutdown_tx.subscribe(),
                        sub_count,
                        stats_delta_tx.clone(),
                        SubscriberMediaConfig {
                            has_video,
                            has_audio,
                            video_width: self.config.video_width,
                            video_height: self.config.video_height,
                            output_group_duration_ms: self.config.output_group_duration_ms,
                            output_initial_delay_ms: self.config.output_initial_delay_ms,
                        },
                        media_state_rx.clone(),
                    ).await {
                        Ok(_handle) => {
                            tracing::info!("Subscriber task started");
                        }
                        Err(e) => {
                            tracing::error!("Failed to start subscriber task: {}", e);
                        }
                    }
                }

                // Forward packets from pin "in" to broadcast channel
                result = async {
                    if let Some(ref mut rx) = pin_0_rx { rx.recv().await } else { std::future::pending().await }
                } => {
                    if let Some(packet) = result {
                        // Lazily determine kind on first packet when input_types
                        // is unavailable (dynamic pipelines).
                        let kind = pin_0_kind.unwrap_or_else(|| {
                            let k = infer_kind_from_packet(&packet);
                            pin_0_kind = Some(k);
                            match k {
                                MediaKind::Audio => has_audio = true,
                                MediaKind::Video => has_video = true,
                            }
                            // In dynamic mode, resolve immediately on first
                            // packet — the subscriber applies a grace period.
                            let resolved = if dynamic_mode {
                                true
                            } else {
                                let pin_1_connected = pin_1_rx.is_some();
                                pin_1_kind.is_some() || !pin_1_connected
                            };
                            let _ = media_state_tx.send(MediaTypeState {
                                has_audio,
                                has_video,
                                resolved,
                            });
                            tracing::info!(?k, resolved, "pin \"in\": media kind inferred from first packet");
                            k
                        });
                        if let Some(frame) = make_broadcast_frame(packet, kind) {
                            stats_tracker.received();
                            let _ = subscriber_broadcast_tx.send(frame);
                            stats_tracker.sent();
                            stats_tracker.maybe_send();
                        }
                    } else {
                        tracing::info!("Pipeline input pin \"in\" closed");
                        pin_0_rx = None;
                        if pin_1_rx.is_none() {
                            tracing::info!("All pipeline inputs closed, shutting down");
                            break Ok(());
                        }
                    }
                }

                // Forward packets from pin "in_1" to broadcast channel
                result = async {
                    if let Some(ref mut rx) = pin_1_rx { rx.recv().await } else { std::future::pending().await }
                } => {
                    if let Some(packet) = result {
                        let kind = pin_1_kind.unwrap_or_else(|| {
                            let k = infer_kind_from_packet(&packet);
                            pin_1_kind = Some(k);
                            match k {
                                MediaKind::Audio => has_audio = true,
                                MediaKind::Video => has_video = true,
                            }
                            let resolved = if dynamic_mode {
                                true
                            } else {
                                let pin_0_connected = pin_0_rx.is_some();
                                pin_0_kind.is_some() || !pin_0_connected
                            };
                            let _ = media_state_tx.send(MediaTypeState {
                                has_audio,
                                has_video,
                                resolved,
                            });
                            tracing::info!(?k, resolved, "pin \"in_1\": media kind inferred from first packet");
                            k
                        });
                        if let Some(frame) = make_broadcast_frame(packet, kind) {
                            stats_tracker.received();
                            let _ = subscriber_broadcast_tx.send(frame);
                            stats_tracker.sent();
                            stats_tracker.maybe_send();
                        }
                    } else {
                        tracing::info!("Pipeline input pin \"in_1\" closed");
                        pin_1_rx = None;
                        if pin_0_rx.is_none() {
                            tracing::info!("All pipeline inputs closed, shutting down");
                            break Ok(());
                        }
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

                // Dynamic pin management — handle engine requests for
                // track-named output pins (e.g. `audio/data`, `video/hd`).
                Some(msg) = async {
                    match &mut pin_mgmt_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    Self::handle_pin_management(msg, &dynamic_outputs, &subscriber_broadcast_tx, &stats_delta_tx, &shutdown_tx, &mut forwarder_handles);
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

        // Abort any lingering dynamic input forwarder tasks and wait briefly
        // for them to finish so the node is fully stopped before we return.
        for (name, handle) in forwarder_handles.drain() {
            handle.abort();
            tracing::debug!(pin = %name, "Aborted dynamic input forwarder");
        }

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
            subscriber_count.load(Ordering::Relaxed)
        );
        final_result
    }
}

/// Create a dynamic input pin that accepts both Opus audio and VP9 video.
fn make_dynamic_input_pin(name: &str) -> InputPin {
    InputPin {
        name: name.to_string(),
        accepts_types: super::constants::moq_accepted_media_types(),
        cardinality: PinCardinality::One,
    }
}

/// Pin names containing a `video/` segment produce [`PacketType::EncodedVideo`] (VP9);
/// all others produce [`PacketType::EncodedAudio`] (Opus). This matches the
/// convention in `MoqPullNode::output_pins_for_tracks`.
///
/// Handles both unprefixed names (`video/hd`) and broadcast-prefixed names
/// (`screen-input/video/hd`) by checking `starts_with("video/")` OR
/// `contains("/video/")`.
///
/// The string-based inference relies on the naming convention enforced by
/// [`watch_catalog_and_process_inner`], which constructs pin names as
/// `{prefix}/{track_name}` where `track_name` always begins with `audio/`
/// or `video/`.  Broadcast prefixes are simple identifiers (e.g.
/// `screen-input`, `cam-input`) so a false-positive match on `/video/` in
/// the prefix portion is not possible in practice.
fn make_dynamic_output_pin(name: &str) -> OutputPin {
    let is_video = name.starts_with("video/") || name.contains("/video/");
    let produces_type = if is_video {
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        })
    } else {
        PacketType::EncodedAudio(EncodedAudioFormat {
            codec: AudioCodec::Opus,
            codec_private: None,
        })
    };
    OutputPin { name: name.to_string(), produces_type, cardinality: PinCardinality::Broadcast }
}

impl MoqPeerNode {
    /// Spawn a forwarding task for a newly added dynamic input pin.
    ///
    /// Packets arriving on `channel` are forwarded into the subscriber
    /// broadcast channel.  The task shuts down cleanly when `shutdown_tx`
    /// fires or the channel closes.
    ///
    /// Returns the [`JoinHandle`] so the caller can abort the task on pin removal.
    fn spawn_dynamic_input_forwarder(
        pin_name: String,
        mut channel: mpsc::Receiver<Packet>,
        subscriber_broadcast_tx: &broadcast::Sender<BroadcastFrame>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        shutdown_tx: &broadcast::Sender<()>,
    ) -> tokio::task::JoinHandle<()> {
        let tx = subscriber_broadcast_tx.clone();
        let stats_tx = stats_delta_tx.clone();
        let mut task_shutdown = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut kind: Option<MediaKind> = None;
            loop {
                tokio::select! {
                    biased;
                    _ = task_shutdown.recv() => {
                        tracing::info!(pin = %pin_name, "Dynamic input forwarding task shutting down");
                        break;
                    }
                    maybe_packet = channel.recv() => {
                        let Some(packet) = maybe_packet else { break };
                        let k = *kind.get_or_insert_with(|| infer_kind_from_packet(&packet));
                        if let Some(frame) = make_broadcast_frame(packet, k) {
                            let _ = stats_tx
                                .try_send(NodeStatsDelta { received: 1, ..Default::default() });
                            // No active subscribers — discard the frame but keep
                            // the forwarder alive so future subscribers receive
                            // data (matches the static input path behaviour).
                            let _ = tx.send(frame);
                            let _ =
                                stats_tx.try_send(NodeStatsDelta { sent: 1, ..Default::default() });
                        }
                    }
                }
            }
            tracing::info!(pin = %pin_name, "Dynamic input pin forwarding task ended");
        })
    }

    /// Insert a channel into the dynamic outputs map.
    ///
    /// Recovers from lock poisoning — see [`DynamicOutputs`] doc comment.
    fn insert_dynamic_output(
        dynamic_outputs: &DynamicOutputs,
        name: String,
        channel: mpsc::Sender<Packet>,
    ) {
        dynamic_outputs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name, channel);
    }

    /// Remove a channel from the dynamic outputs map.
    ///
    /// Recovers from lock poisoning — see [`DynamicOutputs`] doc comment.
    fn remove_dynamic_output(dynamic_outputs: &DynamicOutputs, name: &str) {
        dynamic_outputs.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name);
    }

    /// Handle a dynamic pin management message from the engine.
    ///
    /// - [`PinManagementMessage::RequestAddOutputPin`]: the engine is creating a
    ///   track-named output pin because a downstream node connected to it. We
    ///   respond with an appropriate pin definition.
    /// - [`PinManagementMessage::AddedOutputPin`]: the engine has set up the pin
    ///   distributor and sends us the channel to write frames to.
    fn handle_pin_management(
        msg: PinManagementMessage,
        dynamic_outputs: &DynamicOutputs,
        subscriber_broadcast_tx: &broadcast::Sender<BroadcastFrame>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        shutdown_tx: &broadcast::Sender<()>,
        forwarder_handles: &mut std::collections::HashMap<String, tokio::task::JoinHandle<()>>,
    ) {
        match msg {
            PinManagementMessage::RequestAddOutputPin { suggested_name, response_tx } => {
                let pin_name = suggested_name.unwrap_or_else(|| "dynamic_out".to_string());
                tracing::info!("MoqPeerNode: creating dynamic output pin '{}'", pin_name);
                let pin = make_dynamic_output_pin(&pin_name);
                let _ = response_tx.send(Ok(pin));
            },
            PinManagementMessage::AddedOutputPin { pin, channel } => {
                tracing::info!("MoqPeerNode: activated dynamic output pin '{}'", pin.name);
                Self::insert_dynamic_output(dynamic_outputs, pin.name, channel);
            },
            PinManagementMessage::RequestAddInputPin { suggested_name, response_tx } => {
                let pin_name = suggested_name.unwrap_or_else(|| "dynamic_in".to_string());
                tracing::info!("MoqPeerNode: creating dynamic input pin '{}'", pin_name);
                let _ = response_tx.send(Ok(make_dynamic_input_pin(&pin_name)));
            },
            PinManagementMessage::AddedInputPin { pin, channel } => {
                Self::activate_dynamic_input_forwarder(
                    pin,
                    channel,
                    subscriber_broadcast_tx,
                    stats_delta_tx,
                    shutdown_tx,
                    forwarder_handles,
                );
            },
            PinManagementMessage::RemoveInputPin { pin_name } => {
                tracing::info!("MoqPeerNode: removed input pin '{}'", pin_name);
                if let Some(handle) = forwarder_handles.remove(&pin_name) {
                    handle.abort();
                }
            },
            PinManagementMessage::RemoveOutputPin { pin_name } => {
                tracing::info!("MoqPeerNode: removed output pin '{}'", pin_name);
                Self::remove_dynamic_output(dynamic_outputs, &pin_name);
            },
        }
    }

    /// Activate a dynamic input pin by spawning a forwarder task and
    /// registering its handle. Prunes finished handles and aborts any
    /// existing forwarder for the same pin name to prevent leaks.
    #[allow(clippy::too_many_arguments)]
    fn activate_dynamic_input_forwarder(
        pin: InputPin,
        channel: mpsc::Receiver<Packet>,
        subscriber_broadcast_tx: &broadcast::Sender<BroadcastFrame>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        shutdown_tx: &broadcast::Sender<()>,
        forwarder_handles: &mut std::collections::HashMap<String, tokio::task::JoinHandle<()>>,
    ) {
        tracing::info!("MoqPeerNode: activated dynamic input pin '{}'", pin.name);
        // Prune finished forwarder handles to avoid unbounded growth
        // from naturally-closed channels whose handles were never
        // explicitly removed via RemoveInputPin.
        forwarder_handles.retain(|_, h| !h.is_finished());
        let handle = Self::spawn_dynamic_input_forwarder(
            pin.name.clone(),
            channel,
            subscriber_broadcast_tx,
            stats_delta_tx,
            shutdown_tx,
        );
        if let Some(old) = forwarder_handles.insert(pin.name, handle) {
            old.abort();
            tracing::debug!("Aborted previous forwarder for re-added pin");
        }
    }

    /// Start a task to handle publisher connection (receives media from client)
    // Pin-specific output routing requires per-pin parameters; bundling into a config struct is a future cleanup.
    #[allow(clippy::too_many_arguments)]
    async fn start_publisher_task_with_permit(
        moq_connection: streamkit_core::moq_gateway::MoqConnection,
        permit: OwnedSemaphorePermit,
        input_broadcast: String,
        output_sender: streamkit_core::OutputSender,
        mut shutdown_rx: broadcast::Receiver<()>,
        publisher_events: mpsc::UnboundedSender<PublisherEvent>,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: DynamicOutputs,
    ) -> Result<tokio::task::JoinHandle<Result<(), StreamKitError>>, StreamKitError> {
        let path = moq_connection.path.clone();

        // Extract the moq-native Request
        let request = *moq_connection
            .session
            .downcast::<moq_native::Request>()
            .map_err(|_| StreamKitError::Runtime("Invalid MoQ request type".to_string()))?;

        // Notify gateway that we accepted the connection
        let _ = moq_connection
            .response_tx
            .send(streamkit_core::moq_gateway::MoqConnectionResult::Accepted);

        // Create origin for receiving from client
        let client_publish_origin = moq_lite::Origin::produce();
        let receive_origin = client_publish_origin.consume();

        // Accept MoQ session (publisher only sends, no server publish needed)
        let session = request
            .with_consume(client_publish_origin)
            .ok()
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
                dynamic_outputs,
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

        // Extract the moq-native Request
        let request = *moq_connection
            .session
            .downcast::<moq_native::Request>()
            .map_err(|_| StreamKitError::Runtime("Invalid MoQ request type".to_string()))?;

        // Notify gateway that we accepted the connection
        let _ = moq_connection
            .response_tx
            .send(streamkit_core::moq_gateway::MoqConnectionResult::Accepted);

        // Create origins for full bidirectional MoQ
        let server_publish_origin = moq_lite::Origin::produce();
        let send_origin = server_publish_origin.clone();

        let client_publish_origin = moq_lite::Origin::produce();
        let receive_origin = client_publish_origin.consume();

        let session = request
            .with_publish(server_publish_origin.consume())
            .with_consume(client_publish_origin)
            .ok()
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Failed to accept session: {e}")))?;

        let handle = tokio::spawn(async move {
            let count = config.subscriber_count.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!(path = %path, "Peer connected (total: {})", count);

            let mut publisher_shutdown_rx = config.shutdown_rx.resubscribe();
            let mut subscriber_shutdown_rx = config.shutdown_rx.resubscribe();
            let extra_shutdown_rx = config.shutdown_rx;

            // Clone stats_delta_tx before async blocks to avoid borrow conflicts
            let publisher_stats_delta_tx = config.stats_delta_tx.clone();
            let subscriber_stats_delta_tx = config.stats_delta_tx.clone();
            let extra_stats_delta_tx = config.stats_delta_tx;

            // Create per-broadcast OriginConsumer clones BEFORE moving
            // receive_origin into the primary loop.
            let extra_consumers: Vec<(String, moq_lite::OriginConsumer)> =
                if config.input_broadcasts.len() > 1 {
                    config.input_broadcasts[1..]
                        .iter()
                        .map(|bc| (bc.clone(), receive_origin.consume()))
                        .collect()
                } else {
                    Vec::new()
                };

            let publisher_fut = async {
                // Primary broadcast receive loop
                let primary_broadcast =
                    config.input_broadcasts.first().cloned().unwrap_or_else(|| "input".to_string());
                Self::publisher_receive_loop_with_slot(
                    PublisherReceiveLoopWithSlotConfig {
                        subscribe: receive_origin,
                        broadcast_name: primary_broadcast,
                        output_sender: config.output_sender.clone(),
                        publisher_slot: config.publisher_slot,
                        publisher_events: config.publisher_events,
                        publisher_path: path.clone(),
                        stats_delta_tx: publisher_stats_delta_tx.clone(),
                        dynamic_outputs: config.dynamic_outputs.clone(),
                    },
                    &mut publisher_shutdown_rx,
                )
                .await
            };

            // Spawn additional broadcast watchers for multi-broadcast mode.
            // Each additional broadcast gets its own catalog watcher with
            // namespaced output pins ({broadcast_name}/{track_name}).
            // Task handles are kept so we can abort them when the session ends.
            let extra_handles: Vec<tokio::task::JoinHandle<Result<(), StreamKitError>>> = {
                let mut handles = Vec::new();
                for (bc_name, consumer) in extra_consumers {
                    let output_sender = config.output_sender.clone();
                    let mut shutdown = extra_shutdown_rx.resubscribe();
                    let stats = extra_stats_delta_tx.clone();
                    let dyn_out = config.dynamic_outputs.clone();

                    handles.push(tokio::spawn(async move {
                        tracing::info!(broadcast = %bc_name, "Subscribing to additional broadcast");

                        let Some(broadcast_consumer) =
                            Self::wait_for_broadcast_announcement(
                                consumer,
                                &bc_name,
                                &mut shutdown,
                            )
                            .await?
                        else {
                            return Ok(());
                        };

                        tracing::info!(broadcast = %bc_name, "Additional broadcast announced, watching catalog");

                        Self::watch_catalog_and_process_inner(
                            &broadcast_consumer,
                            output_sender,
                            &mut shutdown,
                            &stats,
                            &dyn_out,
                            Some(&bc_name),
                        )
                        .await
                    }));
                }
                handles
            };

            let subscriber_fut = async {
                Self::subscriber_send_loop(
                    send_origin,
                    config.output_broadcast,
                    config.node_id.clone(),
                    config.broadcast_rx,
                    &mut subscriber_shutdown_rx,
                    subscriber_stats_delta_tx,
                    config.media,
                    config.media_state_rx,
                )
                .await
            };

            // Run publisher and subscriber concurrently — they define the
            // session lifetime.  Extra broadcast tasks run alongside but are
            // cancelled when the session ends to prevent zombie accumulation
            // across reconnections (allow_reconnect: true).
            let (publisher_result, subscriber_result) = tokio::join!(publisher_fut, subscriber_fut);

            // Session ended — abort any still-running extra broadcast tasks.
            for handle in &extra_handles {
                handle.abort();
            }

            // Collect extra broadcast results (cancelled tasks are expected).
            let mut extra_first_error: Option<StreamKitError> = None;
            for handle in extra_handles {
                match handle.await {
                    Err(e) if e.is_cancelled() => {
                        tracing::debug!("Extra broadcast task cancelled (session ended)");
                    },
                    Err(e) => {
                        if extra_first_error.is_none() {
                            extra_first_error = Some(StreamKitError::Runtime(format!(
                                "Multi-broadcast task panicked: {e}"
                            )));
                        }
                    },
                    Ok(Err(e)) => {
                        if extra_first_error.is_none() {
                            extra_first_error = Some(e);
                        }
                    },
                    Ok(Ok(())) => {},
                }
            }

            if let Err(e) = publisher_result {
                tracing::warn!(path = %path, error = %e, "Peer publisher task error");
            }
            if let Err(e) = subscriber_result {
                tracing::warn!(path = %path, error = %e, "Peer subscriber task error");
            }
            if let Some(e) = extra_first_error {
                tracing::warn!(path = %path, error = %e, "Peer multi-broadcast task error");
            }

            let count = config.subscriber_count.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
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

        let result = Self::watch_catalog_and_process(
            &broadcast_consumer,
            config.output_sender,
            shutdown_rx,
            &config.stats_delta_tx,
            &config.dynamic_outputs,
        )
        .await;

        drop(permit);
        let _ = config.publisher_events.send(PublisherEvent::Disconnected {
            path: config.publisher_path,
            error: result.as_ref().err().map(std::string::ToString::to_string),
        });

        result
    }

    /// Publisher receive loop - receives audio/video from client and sends to pipeline
    async fn publisher_receive_loop(
        subscribe: moq_lite::OriginConsumer,
        broadcast_name: String,
        output_sender: streamkit_core::OutputSender,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: DynamicOutputs,
    ) -> Result<(), StreamKitError> {
        tracing::info!("Waiting for publisher to announce broadcast: {}", broadcast_name);

        // Wait for client to announce the broadcast
        let Some(broadcast_consumer) =
            Self::wait_for_broadcast_announcement(subscribe, &broadcast_name, shutdown_rx).await?
        else {
            return Ok(()); // Shutdown requested
        };

        // Watch catalog and process tracks as they appear (handles incremental
        // permission grants where mic/camera become available at different times)
        Self::watch_catalog_and_process(
            &broadcast_consumer,
            output_sender,
            shutdown_rx,
            &stats_delta_tx,
            &dynamic_outputs,
        )
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

    /// Unwrap a catalog read result, returning `Some(catalog)` on success,
    /// or `None` when the caller should break (closed / timeout / error).
    fn unwrap_catalog_result<E: std::fmt::Display>(
        result: Result<Result<Option<hang::catalog::Catalog>, E>, tokio::time::error::Elapsed>,
    ) -> Option<hang::catalog::Catalog> {
        match result {
            Ok(Ok(Some(catalog))) => Some(catalog),
            Ok(Ok(None)) => {
                tracing::info!("Catalog track closed");
                None
            },
            Ok(Err(e)) => {
                tracing::warn!("Error reading catalog: {}", e);
                None
            },
            Err(_) => {
                tracing::info!("Catalog timeout — proceeding with discovered tracks");
                None
            },
        }
    }

    /// Check whether the catalog watch loop has discovered enough tracks to stop.
    ///
    /// In multi-broadcast mode (`is_additional == true`), each additional broadcast
    /// may carry only one media type (e.g. video-only camera input), so we exit as
    /// soon as any track is found.  For the primary broadcast we wait for both
    /// audio and video tracks before stopping.  If the catalog genuinely only
    /// carries one media type the caller handles this via a shortened grace
    /// timeout (see [`Self::watch_catalog_and_process_inner`]).
    fn has_expected_tracks(
        track_handles: &std::collections::HashMap<
            String,
            tokio::task::JoinHandle<Result<(), StreamKitError>>,
        >,
        is_additional: bool,
    ) -> bool {
        if track_handles.is_empty() {
            return false;
        }
        if is_additional {
            return true;
        }
        let has_audio = track_handles.keys().any(|k| k.starts_with("audio/"));
        let has_video = track_handles.keys().any(|k| k.starts_with("video/"));
        has_audio && has_video
    }

    /// Watch the catalog continuously and process publisher tracks as they appear.
    ///
    /// Instead of waiting for all tracks upfront, this subscribes to and starts
    /// processing each track as soon as it appears in the catalog. This handles
    /// the common case where the browser grants mic and camera permissions at
    /// different times, causing the hang library to publish incremental catalog
    /// updates (e.g., audio-only first, then audio+video).
    ///
    /// Supports N tracks per broadcast — each rendition in the catalog gets its
    /// own track processor task, keyed by track name in a `HashMap`.
    async fn watch_catalog_and_process(
        broadcast_consumer: &moq_lite::BroadcastConsumer,
        output_sender: streamkit_core::OutputSender,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: &DynamicOutputs,
    ) -> Result<(), StreamKitError> {
        Self::watch_catalog_and_process_inner(
            broadcast_consumer,
            output_sender,
            shutdown_rx,
            stats_delta_tx,
            dynamic_outputs,
            None, // no pin prefix for single-broadcast mode
        )
        .await
    }

    /// Subscribe to newly discovered tracks from a catalog update.
    ///
    /// For each audio and video rendition in the catalog that isn't already
    /// being handled, spawns a track processor task with the appropriate
    /// output pin name (optionally prefixed for multi-broadcast mode).
    #[allow(clippy::too_many_arguments)]
    fn subscribe_catalog_tracks(
        catalog: &hang::catalog::Catalog,
        broadcast_consumer: &moq_lite::BroadcastConsumer,
        output_sender: &streamkit_core::OutputSender,
        shutdown_rx: &broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: &DynamicOutputs,
        pin_prefix: Option<&str>,
        track_handles: &mut std::collections::HashMap<
            String,
            tokio::task::JoinHandle<Result<(), StreamKitError>>,
        >,
    ) {
        // Subscribe to all audio tracks not yet being handled
        for track_name in catalog.audio.renditions.keys() {
            if !track_handles.contains_key(track_name) {
                tracing::info!("Found audio track in catalog: {}", track_name);
                let output_pin = pin_prefix
                    .map_or_else(|| track_name.clone(), |prefix| format!("{prefix}/{track_name}"));
                track_handles.insert(
                    track_name.clone(),
                    Self::spawn_track_processor_with_pin(
                        broadcast_consumer,
                        track_name,
                        false,
                        output_sender,
                        shutdown_rx,
                        stats_delta_tx,
                        dynamic_outputs,
                        &output_pin,
                    ),
                );
            }
        }

        // Subscribe to all video tracks not yet being handled
        for track_name in catalog.video.renditions.keys() {
            if !track_handles.contains_key(track_name) {
                tracing::info!("Found video track in catalog: {}", track_name);
                let output_pin = pin_prefix
                    .map_or_else(|| track_name.clone(), |prefix| format!("{prefix}/{track_name}"));
                track_handles.insert(
                    track_name.clone(),
                    Self::spawn_track_processor_with_pin(
                        broadcast_consumer,
                        track_name,
                        true,
                        output_sender,
                        shutdown_rx,
                        stats_delta_tx,
                        dynamic_outputs,
                        &output_pin,
                    ),
                );
            }
        }
    }

    /// Inner catalog watch loop shared by single- and multi-broadcast modes.
    ///
    /// When `pin_prefix` is `Some(name)`, output pin names are namespaced as
    /// `{name}/{track_name}` (e.g. `screen-input/video/hd`).  When `None`,
    /// track names are used directly (e.g. `video/hd`).
    async fn watch_catalog_and_process_inner(
        broadcast_consumer: &moq_lite::BroadcastConsumer,
        output_sender: streamkit_core::OutputSender,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: &DynamicOutputs,
        pin_prefix: Option<&str>,
    ) -> Result<(), StreamKitError> {
        let catalog_track =
            broadcast_consumer.subscribe_track(&hang::catalog::Catalog::default_track()).map_err(
                |e| StreamKitError::Runtime(format!("Failed to subscribe to catalog track: {e}")),
            )?;
        let mut catalog_consumer = hang::catalog::CatalogConsumer::new(catalog_track);

        let mut track_handles: std::collections::HashMap<
            String,
            tokio::task::JoinHandle<Result<(), StreamKitError>>,
        > = std::collections::HashMap::new();

        // After the first tracks are discovered we switch from the initial 30s
        // timeout to a shorter 5s grace period.  This handles the common case
        // where the browser grants mic and camera permissions at different
        // times: the first catalog arrives with audio only, and we keep
        // watching briefly for the second catalog that adds video.  If no
        // additional catalog arrives within the grace period we conclude that
        // the publisher genuinely only has one media type, avoiding the full
        // 30-second wait that the old code suffered.
        let mut tracks_discovered = false;

        // Monitor the catalog for new tracks, subscribing to each as it appears
        loop {
            let timeout_secs = if tracks_discovered { 5 } else { 30 };
            tokio::select! {
                biased;
                _ = shutdown_rx.recv() => {
                    tracing::info!("Catalog watch shutting down");
                    break;
                }
                catalog_result = tokio::time::timeout(Duration::from_secs(timeout_secs), catalog_consumer.next()) => {
                    let Some(catalog) = Self::unwrap_catalog_result(catalog_result) else {
                        break;
                    };

                    tracing::info!(
                        "Received catalog from publisher: audio={:?}, video renditions={}",
                        catalog.audio, catalog.video.renditions.len()
                    );

                    Self::subscribe_catalog_tracks(
                        &catalog,
                        broadcast_consumer,
                        &output_sender,
                        shutdown_rx,
                        stats_delta_tx,
                        dynamic_outputs,
                        pin_prefix,
                        &mut track_handles,
                    );

                    let is_additional_broadcast = pin_prefix.is_some();
                    if Self::has_expected_tracks(&track_handles, is_additional_broadcast) {
                        tracing::info!(
                            additional = is_additional_broadcast,
                            "Expected tracks discovered, stopping catalog watch"
                        );
                        break;
                    }

                    // Not all media types found yet — if we have some tracks,
                    // switch to the shorter grace timeout for the next iteration
                    // so we don't wait the full 30s for a genuinely single-media
                    // pipeline.
                    if !track_handles.is_empty() && !tracks_discovered {
                        tracks_discovered = true;
                        tracing::info!(
                            "Tracks discovered but not all media types present — \
                             switching to {}s grace period for incremental catalogs",
                            5
                        );
                    }
                }
            }
        }

        // Wait for all active processing tasks to finish
        Self::await_track_tasks_map(track_handles).await
    }

    /// Spawn a track processor with a custom output pin name.
    ///
    /// This is the multi-broadcast variant where the output pin name may be
    /// namespaced (e.g. `screen-input/video/hd`).
    #[allow(clippy::too_many_arguments)]
    fn spawn_track_processor_with_pin(
        broadcast_consumer: &moq_lite::BroadcastConsumer,
        track_name: &str,
        is_video: bool,
        output_sender: &streamkit_core::OutputSender,
        shutdown_rx: &broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: &DynamicOutputs,
        output_pin_name: &str,
    ) -> tokio::task::JoinHandle<Result<(), StreamKitError>> {
        const MAX_RESUBSCRIBE_ATTEMPTS: u32 = 10;
        const RESUBSCRIBE_INITIAL_BACKOFF: Duration = Duration::from_millis(100);

        let broadcast = broadcast_consumer.clone();
        let track = moq_lite::Track { name: track_name.to_string(), priority: 2 };
        let sender = output_sender.clone();
        let mut task_shutdown = shutdown_rx.resubscribe();
        let stats = stats_delta_tx.clone();
        let output_pin = output_pin_name.to_string();
        let dyn_outputs = dynamic_outputs.clone();

        tokio::spawn(async move {
            tracing::info!(output_pin = %output_pin, track = %track.name, "Track processor task started");

            let mut attempt: u32 = 0;
            loop {
                let consumer = broadcast.subscribe_track(&track).map_err(|e| {
                    StreamKitError::Runtime(format!(
                        "Failed to subscribe to track '{}': {e}",
                        track.name
                    ))
                })?;
                let exit = Self::process_publisher_frames(
                    consumer,
                    sender.clone(),
                    &output_pin,
                    is_video,
                    &mut task_shutdown,
                    &stats,
                    &dyn_outputs,
                )
                .await;

                match exit {
                    TrackExit::Finished => {
                        tracing::info!(
                            output_pin = %output_pin,
                            "Track processor task finished normally"
                        );
                        return Ok(());
                    },
                    TrackExit::Error(e) => {
                        tracing::warn!(
                            output_pin = %output_pin,
                            error = %e,
                            "Track processor task finished with error"
                        );
                        return Err(e);
                    },
                    TrackExit::Cancelled => {
                        attempt += 1;
                        if attempt > MAX_RESUBSCRIBE_ATTEMPTS {
                            tracing::warn!(
                                output_pin = %output_pin,
                                attempts = attempt,
                                "Publisher track cancelled; retry budget exhausted"
                            );
                            return Err(StreamKitError::Runtime(format!(
                                "publisher track '{}' cancelled {} times; giving up",
                                track.name, attempt
                            )));
                        }
                        let backoff =
                            RESUBSCRIBE_INITIAL_BACKOFF * 2u32.saturating_pow(attempt - 1);
                        tracing::info!(
                            output_pin = %output_pin,
                            attempt,
                            max = MAX_RESUBSCRIBE_ATTEMPTS,
                            backoff_ms = backoff.as_millis(),
                            "Publisher track cancelled; re-subscribing after backoff"
                        );
                        tokio::select! {
                            biased;
                            _ = task_shutdown.recv() => {
                                tracing::info!(output_pin = %output_pin, "Shutdown during re-subscribe backoff");
                                return Ok(());
                            }
                            () = tokio::time::sleep(backoff) => {}
                        }
                    },
                }
            }
        })
    }

    /// Wait for spawned track processing tasks to complete.
    ///
    /// Generalized version that handles N track processor tasks stored in a
    /// `HashMap`.  Uses `futures::future::join_all` to wait for all tasks
    /// concurrently and returns the first error encountered (if any).
    async fn await_track_tasks_map(
        track_handles: std::collections::HashMap<
            String,
            tokio::task::JoinHandle<Result<(), StreamKitError>>,
        >,
    ) -> Result<(), StreamKitError> {
        if track_handles.is_empty() {
            tracing::warn!("Publisher catalog had no audio or video tracks");
            return Ok(());
        }

        let names_and_handles: Vec<(String, tokio::task::JoinHandle<Result<(), StreamKitError>>)> =
            track_handles.into_iter().collect();
        let (names, handles): (Vec<_>, Vec<_>) = names_and_handles.into_iter().unzip();

        let results = futures::future::join_all(handles).await;

        let mut first_error: Option<StreamKitError> = None;
        for (track_name, result) in names.into_iter().zip(results) {
            match result {
                Err(e) => {
                    tracing::warn!(track = %track_name, "Track task panicked: {e}");
                    if first_error.is_none() {
                        first_error = Some(StreamKitError::Runtime(format!(
                            "Track task '{track_name}' panicked: {e}"
                        )));
                    }
                },
                Ok(Err(e)) => {
                    tracing::warn!(track = %track_name, "Track task error: {e}");
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                },
                Ok(Ok(())) => {
                    tracing::info!(track = %track_name, "Track processing task completed");
                },
            }
        }

        first_error.map_or(Ok(()), Err)
    }

    /// Process incoming frames from the publisher and forward to the pipeline.
    ///
    /// Returns [`TrackExit`] so the caller can distinguish a transient
    /// publisher-side cancellation (retryable via re-subscribe) from clean
    /// completion and fatal errors.
    // Dynamic output routing adds parameters beyond the static-pin version.
    #[allow(clippy::too_many_arguments)]
    async fn process_publisher_frames(
        mut track_consumer: moq_lite::TrackConsumer,
        mut output_sender: streamkit_core::OutputSender,
        output_pin: &str,
        is_video: bool,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: &DynamicOutputs,
    ) -> TrackExit {
        let mut frame_count = 0u64;
        let mut last_log = std::time::Instant::now();
        let mut current_group: Option<moq_lite::GroupConsumer> = None;
        // Tracks whether the next frame is the first in a new MoQ group.
        // In the hang protocol each group starts with a keyframe.
        let mut is_first_in_group = false;

        loop {
            // Get a group if we don't have one
            if current_group.is_none() {
                match Self::get_next_group(&mut track_consumer, shutdown_rx, output_pin).await {
                    Ok(Some(group)) => {
                        current_group = Some(group);
                        is_first_in_group = true;
                    },
                    Ok(None) => return TrackExit::Finished, // Stream ended or shutdown
                    Err(moq_lite::Error::Cancel) => return TrackExit::Cancelled,
                    Err(e) => {
                        return TrackExit::Error(StreamKitError::Runtime(format!(
                            "Error getting group: {e}"
                        )));
                    },
                }
            }

            // Process frames from current group
            if let Some(ref mut group) = current_group {
                let keyframe = is_first_in_group;
                is_first_in_group = false;
                match Self::process_frame_from_group(
                    group,
                    &mut output_sender,
                    output_pin,
                    is_video,
                    &mut frame_count,
                    &mut last_log,
                    shutdown_rx,
                    stats_delta_tx,
                    keyframe,
                    dynamic_outputs,
                )
                .await
                {
                    Ok(FrameResult::Continue) => {},
                    Ok(FrameResult::GroupExhausted) => current_group = None,
                    Ok(FrameResult::Shutdown) => return TrackExit::Finished,
                    Err(e) => return TrackExit::Error(e),
                }
            }
        }
    }

    /// Get the next group from the track consumer.
    ///
    /// Surfaces the raw [`moq_lite::Error`] so the caller can distinguish
    /// [`moq_lite::Error::Cancel`] (publisher dropped the track producer —
    /// retryable) from other failures. The `tracing::warn!` here will still
    /// fire for cancellations; that's intentional since they're unexpected
    /// in steady state even if we recover.
    async fn get_next_group(
        track_consumer: &mut moq_lite::TrackConsumer,
        shutdown_rx: &mut broadcast::Receiver<()>,
        output_pin: &str,
    ) -> Result<Option<moq_lite::GroupConsumer>, moq_lite::Error> {
        tokio::select! {
            biased;
            group_result = track_consumer.next_group() => {
                match group_result {
                    Ok(Some(group)) => {
                        tracing::debug!(output_pin, "Got next group from publisher");
                        Ok(Some(group))
                    }
                    Ok(None) => {
                        tracing::info!(output_pin, "Publisher stream ended (next_group returned None)");
                        Ok(None)
                    }
                    Err(e) => {
                        tracing::warn!(output_pin, error = %e, "Error getting group from publisher");
                        Err(e)
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!(output_pin, "Publisher receive loop shutting down (shutdown signal)");
                Ok(None)
            }
        }
    }

    /// Route a packet to the appropriate output channel.
    ///
    /// If a dynamic output channel exists for `output_pin` (created on-demand
    /// by the engine when a downstream node connects to a track-named pin),
    /// the packet is sent there exclusively.  Otherwise it falls through to the
    /// static `output_sender` which handles declared output pins.
    ///
    /// Returns `true` if the send succeeded (or was handled dynamically),
    /// `false` if the static output channel is closed (caller should shut down).
    async fn route_packet(
        packet: Packet,
        output_pin: &str,
        output_sender: &mut streamkit_core::OutputSender,
        dynamic_outputs: &DynamicOutputs,
    ) -> bool {
        // Acquire the lock once and perform both the existence check and the
        // send under the same guard to avoid double acquisition on every packet.
        //
        // The enum tracks what happened so we can act after releasing the lock:
        //   Sent       — packet was forwarded to the dynamic channel
        //   Dropped    — channel full, packet dropped (real-time media trade-off)
        //   Closed     — dynamic channel exists but is closed (stale entry)
        //   NoEntry(p) — no dynamic channel; packet returned for static path
        enum RouteOutcome {
            Sent,
            Dropped,
            Closed,
            NoEntry(Packet),
        }

        let outcome = {
            // Recover from poisoning — the HashMap data is still valid even if
            // another thread panicked while holding the lock.
            let map = dynamic_outputs.read().unwrap_or_else(std::sync::PoisonError::into_inner);
            match map.get(output_pin) {
                Some(dyn_tx) if dyn_tx.is_closed() => RouteOutcome::Closed,
                Some(dyn_tx) => match dyn_tx.try_send(packet) {
                    Ok(()) => RouteOutcome::Sent,
                    Err(mpsc::error::TrySendError::Full(_)) => RouteOutcome::Dropped,
                    Err(mpsc::error::TrySendError::Closed(_)) => RouteOutcome::Closed,
                },
                None => RouteOutcome::NoEntry(packet),
            }
        };

        match outcome {
            RouteOutcome::Sent => true,
            RouteOutcome::Dropped => {
                tracing::debug!(output_pin, "Dynamic output channel full, packet dropped");
                true
            },
            RouteOutcome::Closed => {
                // Downstream consumer disconnected — remove the stale entry
                // but keep the track processor alive so other consumers (or a
                // reconnecting one) can still receive frames.
                tracing::info!(output_pin, "Dynamic output channel closed, removing stale entry");
                Self::remove_dynamic_output(dynamic_outputs, output_pin);
                true
            },
            RouteOutcome::NoEntry(packet) => {
                // No dynamic channel — fall through to the static output sender.
                output_sender.send(output_pin, packet).await.is_ok()
            },
        }
    }

    /// Process a single frame from the current group.
    ///
    /// `is_keyframe` indicates whether this is the first frame of a new MoQ
    /// group, which in the hang protocol corresponds to a keyframe boundary.
    #[allow(clippy::too_many_arguments)]
    async fn process_frame_from_group(
        group: &mut moq_lite::GroupConsumer,
        output_sender: &mut streamkit_core::OutputSender,
        output_pin: &str,
        is_video: bool,
        frame_count: &mut u64,
        last_log: &mut std::time::Instant,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        is_keyframe: bool,
        dynamic_outputs: &DynamicOutputs,
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

                        // Decode the hang protocol timestamp (varint-encoded microseconds)
                        // and propagate it as PacketMetadata so downstream nodes have timing.
                        let timestamp = match hang::container::Timestamp::decode(&mut payload) {
                            Ok(ts) => ts,
                            Err(e) => {
                                tracing::warn!("Failed to decode frame timestamp: {e}");
                                let _ = stats_delta_tx
                                    .try_send(NodeStatsDelta { received: 1, discarded: 1, ..Default::default() });
                                return Ok(FrameResult::Continue);
                            },
                        };
                        #[allow(clippy::cast_possible_truncation)] // MoQ timestamps fit in u64
                        let timestamp_us = timestamp.as_micros() as u64;

                        let data = payload.copy_to_bytes(payload.remaining());
                        let content_type = if is_video {
                            Some(std::borrow::Cow::Borrowed("video/vp9"))
                        } else {
                            None
                        };
                        let packet = Packet::Binary {
                            data,
                            content_type,
                            metadata: Some(streamkit_core::types::PacketMetadata {
                                timestamp_us: Some(timestamp_us),
                                duration_us: None,
                                sequence: None,
                                keyframe: Some(is_keyframe),
                            }),
                        };

                        if !Self::route_packet(packet, output_pin, output_sender, dynamic_outputs).await {
                            tracing::info!(output_pin, "Output channel closed for pin");
                            let _ = stats_delta_tx
                                .try_send(NodeStatsDelta { received: 1, ..Default::default() });
                            return Ok(FrameResult::Shutdown);
                        }
                        let _ = stats_delta_tx.try_send(NodeStatsDelta { received: 1, sent: 1, ..Default::default() });
                        Ok(FrameResult::Continue)
                    }
                    Ok(None) => Ok(FrameResult::GroupExhausted),
                    Err(e) => {
                        tracing::warn!(output_pin, "Error reading frame: {e}");
                        let _ = stats_delta_tx.try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                        Ok(FrameResult::GroupExhausted)
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                tracing::info!(output_pin, "Publisher receive loop shutting down (frame read)");
                Ok(FrameResult::Shutdown)
            }
        }
    }

    /// Start a task to handle subscriber connection (sends audio to client)
    #[allow(clippy::too_many_arguments)]
    async fn start_subscriber_task(
        moq_connection: streamkit_core::moq_gateway::MoqConnection,
        node_id: String,
        output_broadcast: String,
        broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        mut shutdown_rx: broadcast::Receiver<()>,
        subscriber_count: Arc<AtomicU64>,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
        media: SubscriberMediaConfig,
        media_state_rx: watch::Receiver<MediaTypeState>,
    ) -> Result<tokio::task::JoinHandle<()>, StreamKitError> {
        // Extract the moq-native Request
        let request = *moq_connection
            .session
            .downcast::<moq_native::Request>()
            .map_err(|_| StreamKitError::Runtime("Invalid MoQ request type".to_string()))?;

        // Notify gateway that we accepted the connection
        let _ = moq_connection
            .response_tx
            .send(streamkit_core::moq_gateway::MoqConnectionResult::Accepted);

        // Create origin for sending to client
        let server_publish_origin = moq_lite::Origin::produce();
        let send_origin = server_publish_origin.clone();

        // Accept MoQ session (subscriber only receives, no client publish needed)
        let session = request
            .with_publish(server_publish_origin.consume())
            .ok()
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Failed to accept session: {e}")))?;

        let handle = tokio::spawn(async move {
            let count = subscriber_count.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::info!("Subscriber connected (total: {})", count);

            let result = Self::subscriber_send_loop(
                send_origin,
                output_broadcast,
                node_id,
                broadcast_rx,
                &mut shutdown_rx,
                stats_delta_tx,
                media,
                media_state_rx,
            )
            .await;

            // Decrement subscriber count
            let count = subscriber_count.fetch_sub(1, Ordering::Relaxed).saturating_sub(1);
            tracing::info!("Subscriber disconnected (remaining: {})", count);

            // Keep session alive until task ends
            drop(session);

            if let Err(e) = result {
                tracing::warn!("Subscriber task error: {}", e);
            }
        });

        Ok(handle)
    }

    /// Wait for media type resolution and an optional grace period for
    /// additional media types, then apply the resolved state to `media`.
    ///
    /// ## Timing contract
    ///
    /// | Phase | Timeout | Purpose |
    /// |-------|---------|---------|
    /// | Resolution wait | 5 s | Wait for at least one input pin to deliver a packet so its media kind is known. Static pipelines resolve immediately; dynamic pipelines wait here. |
    /// | Grace period | 500 ms | After one kind is known, briefly wait for the other kind before publishing the catalog. If the second kind doesn't arrive, the catalog is published with only the known type. |
    ///
    /// In dynamic pipelines both audio and video are optimistically advertised
    /// in the `MediaTypeState`, so the grace period is effectively skipped
    /// (both types are already flagged). The grace period matters only for
    /// static pipelines where exactly one pin has a known type and the other
    /// is connected but hasn't delivered its first packet yet.
    ///
    /// Returns `Ok(true)` when resolution succeeded and the caller should
    /// continue, or `Ok(false)` when a shutdown was received.
    async fn resolve_media_types(
        media: &mut SubscriberMediaConfig,
        media_state_rx: &mut watch::Receiver<MediaTypeState>,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<bool, StreamKitError> {
        // Wait for media types to be resolved before building the catalog.
        // For static pipelines `resolved` is true immediately.  For dynamic
        // pipelines we wait until the first packet on any connected input pin
        // has been processed so we know at least one media type.
        if !media_state_rx.borrow().resolved {
            tracing::info!("Waiting for input pin media types to be resolved...");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                tokio::select! {
                    result = media_state_rx.changed() => {
                        if result.is_err() { break; }
                        if media_state_rx.borrow().resolved { break; }
                    }
                    () = tokio::time::sleep_until(deadline) => {
                        tracing::warn!("Timed out waiting for media type resolution");
                        break;
                    }
                    _recv = shutdown_rx.recv() => {
                        tracing::info!("Shutdown while waiting for media type resolution");
                        return Ok(false);
                    }
                }
            }
        }

        // After resolution, if only partial media types are known, wait a
        // brief grace period for additional types.  In dynamic pipelines a
        // second input pin may receive its first packet shortly after the
        // first pin resolved the state.
        let needs_grace = {
            let state = media_state_rx.borrow();
            state.resolved && !(state.has_audio && state.has_video)
        };
        if needs_grace {
            let grace = tokio::time::Instant::now() + Duration::from_millis(500);
            loop {
                tokio::select! {
                    result = media_state_rx.changed() => {
                        if result.is_err() { break; }
                        let both = {
                            let s = media_state_rx.borrow();
                            s.has_audio && s.has_video
                        };
                        if both { break; }
                    }
                    () = tokio::time::sleep_until(grace) => { break; }
                    _recv = shutdown_rx.recv() => {
                        tracing::info!("Shutdown during media type grace period");
                        return Ok(false);
                    }
                }
            }
        }

        // Apply the resolved media state.
        {
            let state = media_state_rx.borrow();
            media.has_audio = state.has_audio;
            media.has_video = state.has_video;
        }

        Ok(true)
    }

    /// Subscriber send loop - receives from broadcast channel and sends to client
    // media_state_rx adds a necessary parameter for dynamic media-type resolution.
    #[allow(clippy::too_many_arguments)]
    async fn subscriber_send_loop(
        publish: moq_lite::OriginProducer,
        broadcast_name: String,
        node_id: String,
        broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
        mut media: SubscriberMediaConfig,
        mut media_state_rx: watch::Receiver<MediaTypeState>,
    ) -> Result<(), StreamKitError> {
        if !Self::resolve_media_types(&mut media, &mut media_state_rx, shutdown_rx).await? {
            return Ok(());
        }

        // Setup broadcast and tracks
        let (
            _broadcast_producer,
            mut audio_track_producer,
            mut video_track_producer,
            _catalog_producer,
        ) = Self::setup_subscriber_broadcast(&publish, &broadcast_name, &media)?;

        tracing::info!(
            has_audio = media.has_audio,
            has_video = media.has_video,
            "Published catalog to subscriber"
        );

        // Run the send loop
        let packet_count = Self::run_subscriber_send_loop(
            &mut audio_track_producer,
            &mut video_track_producer,
            broadcast_rx,
            shutdown_rx,
            media.output_group_duration_ms,
            media.output_initial_delay_ms,
            node_id,
            broadcast_name,
            &stats_delta_tx,
        )
        .await?;

        if let Some(ref mut p) = audio_track_producer {
            let _ = p.track.finish();
        }
        if let Some(ref mut p) = video_track_producer {
            let _ = p.track.finish();
        }
        tracing::info!("Subscriber task finished after {} packets", packet_count);
        Ok(())
    }

    /// Setup broadcast, media tracks, and catalog for subscriber
    fn setup_subscriber_broadcast(
        publish: &moq_lite::OriginProducer,
        broadcast_name: &str,
        media: &SubscriberMediaConfig,
    ) -> Result<
        (
            moq_lite::BroadcastProducer,
            Option<hang::container::OrderedProducer>,
            Option<hang::container::OrderedProducer>,
            moq_lite::TrackProducer,
        ),
        StreamKitError,
    > {
        // Create broadcast
        let mut broadcast_producer = publish.create_broadcast(broadcast_name).ok_or_else(|| {
            StreamKitError::Runtime(format!("Failed to create broadcast '{broadcast_name}'"))
        })?;

        // Create audio track (if audio input connected)
        let audio_track = if media.has_audio {
            let track = moq_lite::Track { name: "audio/data".to_string(), priority: 80 };
            let producer = broadcast_producer.create_track(track.clone()).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create audio track: {e}"))
            })?;
            Some((track, hang::container::OrderedProducer::from(producer)))
        } else {
            None
        };

        // Create video track (if video input connected)
        let video_track = if media.has_video {
            let track = moq_lite::Track { name: "video/data".to_string(), priority: 60 };
            let producer = broadcast_producer.create_track(track.clone()).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create video track: {e}"))
            })?;
            Some((track, hang::container::OrderedProducer::from(producer)))
        } else {
            None
        };

        // Create and publish catalog
        let catalog_producer = Self::create_and_publish_catalog(
            &mut broadcast_producer,
            audio_track.as_ref().map(|(t, _)| t),
            video_track.as_ref().map(|(t, _)| t),
            media.video_width,
            media.video_height,
        )?;

        Ok((
            broadcast_producer,
            audio_track.map(|(_, p)| p),
            video_track.map(|(_, p)| p),
            catalog_producer,
        ))
    }

    /// Create and publish the catalog with audio and/or video track info
    fn create_and_publish_catalog(
        broadcast_producer: &mut moq_lite::BroadcastProducer,
        audio_track: Option<&moq_lite::Track>,
        video_track: Option<&moq_lite::Track>,
        video_width: u32,
        video_height: u32,
    ) -> Result<moq_lite::TrackProducer, StreamKitError> {
        let mut audio_renditions = std::collections::BTreeMap::new();
        if let Some(audio_track) = audio_track {
            audio_renditions.insert(
                audio_track.name.clone(),
                hang::catalog::AudioConfig {
                    codec: hang::catalog::AudioCodec::Opus,
                    sample_rate: 48000,
                    channel_count: 1,
                    bitrate: Some(64_000),
                    description: None,
                    container: hang::catalog::Container::default(),
                    jitter: None,
                },
            );
        }

        let mut video_renditions = std::collections::BTreeMap::new();
        if let Some(video_track) = video_track {
            video_renditions.insert(
                video_track.name.clone(),
                hang::catalog::VideoConfig {
                    codec: hang::catalog::VideoCodec::VP9(hang::catalog::VP9 {
                        profile: VP9_PROFILE,
                        level: VP9_LEVEL,
                        bit_depth: VP9_BIT_DEPTH,
                        ..hang::catalog::VP9::default()
                    }),
                    coded_width: Some(video_width),
                    coded_height: Some(video_height),
                    display_ratio_width: None,
                    display_ratio_height: None,
                    framerate: Some(30.0),
                    bitrate: None,
                    description: None,
                    optimize_for_latency: Some(true),
                    container: hang::catalog::Container::default(),
                    jitter: None,
                },
            );
        }

        let catalog = hang::catalog::Catalog {
            audio: hang::catalog::Audio { renditions: audio_renditions },
            video: hang::catalog::Video {
                renditions: video_renditions,
                display: None,
                rotation: None,
                flip: None,
            },
            ..Default::default()
        };

        let mut catalog_producer = broadcast_producer
            .create_track(hang::catalog::Catalog::default_track())
            .map_err(|e| StreamKitError::Runtime(format!("Failed to create catalog track: {e}")))?;
        let catalog_json = catalog
            .to_string()
            .map_err(|e| StreamKitError::Runtime(format!("Failed to serialize catalog: {e}")))?;
        catalog_producer
            .write_frame(catalog_json.into_bytes())
            .map_err(|e| StreamKitError::Runtime(format!("Failed to write catalog frame: {e}")))?;

        Ok(catalog_producer)
    }

    /// Run the main send loop, forwarding packets to the subscriber
    #[allow(clippy::too_many_arguments)]
    async fn run_subscriber_send_loop(
        audio_track_producer: &mut Option<hang::container::OrderedProducer>,
        video_track_producer: &mut Option<hang::container::OrderedProducer>,
        mut broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        shutdown_rx: &mut broadcast::Receiver<()>,
        output_group_duration_ms: u64,
        output_initial_delay_ms: u64,
        node_id: String,
        broadcast_name: String,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
    ) -> Result<u64, StreamKitError> {
        let meter = opentelemetry::global::meter("skit_nodes");
        let gap_histogram = meter
            .f64_histogram("moq.peer.inter_frame_ms")
            .with_description("Gap between consecutive frames sent to subscribers")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_FRAME_GAP_MS.to_vec())
            .build();

        let mut ctx = SubscriberSendCtx {
            audio_track_producer,
            video_track_producer,
            packet_count: 0,
            frame_count: 0,
            audio_first_sent: false,
            last_log: std::time::Instant::now(),
            group_duration_ms: output_group_duration_ms.max(1),
            audio_clock: MediaClock::new(output_initial_delay_ms),
            video_clock: MediaClock::new(output_initial_delay_ms),
            gap_histogram,
            metric_labels: [
                opentelemetry::KeyValue::new("node_id", node_id),
                opentelemetry::KeyValue::new("broadcast", broadcast_name),
            ],
            last_audio_ts_ms: None,
            last_video_ts_ms: None,
            stats_delta_tx,
        };

        loop {
            tokio::select! {
                recv_result = broadcast_rx.recv() => {
                    match Self::handle_broadcast_recv(recv_result, &mut ctx)? {
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

        Ok(ctx.packet_count)
    }

    /// Handle a single broadcast receive result, routing to the correct track producer.
    #[allow(clippy::cast_precision_loss)]
    fn handle_broadcast_recv(
        recv_result: Result<BroadcastFrame, broadcast::error::RecvError>,
        ctx: &mut SubscriberSendCtx<'_>,
    ) -> Result<SendResult, StreamKitError> {
        match recv_result {
            Ok(broadcast_frame) => {
                ctx.packet_count += 1;
                ctx.frame_count += 1;

                if ctx.last_log.elapsed() > Duration::from_secs(1) {
                    tracing::debug!("Subscriber: sent {} frames/sec", ctx.frame_count);
                    ctx.frame_count = 0;
                    ctx.last_log = std::time::Instant::now();
                }

                // Select the appropriate clock, last_ts, and track producer based on media kind
                let (clock, last_ts_ms, track_producer) = match broadcast_frame.kind {
                    MediaKind::Audio => (
                        &mut ctx.audio_clock,
                        &mut ctx.last_audio_ts_ms,
                        &mut ctx.audio_track_producer,
                    ),
                    MediaKind::Video => (
                        &mut ctx.video_clock,
                        &mut ctx.last_video_ts_ms,
                        &mut ctx.video_track_producer,
                    ),
                };

                let Some(track_producer) = track_producer else {
                    // No track producer for this media kind — skip frame
                    return Ok(SendResult::Continue);
                };

                let timestamp_ms = clock.timestamp_ms();
                // For audio, use time-based group boundaries; for video, use keyframe flag
                let keyframe = match broadcast_frame.kind {
                    MediaKind::Audio => {
                        let first = !ctx.audio_first_sent;
                        ctx.audio_first_sent = true;
                        first || clock.is_group_boundary_ms(ctx.group_duration_ms)
                    },
                    MediaKind::Video => broadcast_frame.keyframe,
                };

                if let Some(prev) = *last_ts_ms {
                    let gap = timestamp_ms.saturating_sub(prev);
                    ctx.gap_histogram.record(gap as f64, &ctx.metric_labels);
                }
                *last_ts_ms = Some(timestamp_ms);

                let timestamp =
                    hang::container::Timestamp::from_millis(timestamp_ms).map_err(|_| {
                        StreamKitError::Runtime("MoQ frame timestamp overflow".to_string())
                    })?;

                let mut payload = hang::container::BufList::new();
                payload.push_chunk(broadcast_frame.data);

                if keyframe {
                    if let Err(e) = track_producer.keyframe() {
                        tracing::warn!(kind = ?broadcast_frame.kind, "Failed to signal keyframe: {e}");
                        let _ = ctx
                            .stats_delta_tx
                            .try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                        return Ok(SendResult::Stop);
                    }
                }

                let frame = hang::container::Frame { timestamp, payload };

                if let Err(e) = track_producer.write(frame) {
                    tracing::warn!(kind = ?broadcast_frame.kind, "Failed to write MoQ frame to subscriber: {e}");
                    let _ = ctx
                        .stats_delta_tx
                        .try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                    return Ok(SendResult::Stop);
                }

                let default_duration = match broadcast_frame.kind {
                    MediaKind::Audio => super::constants::DEFAULT_AUDIO_FRAME_DURATION_US,
                    MediaKind::Video => crate::video::DEFAULT_VIDEO_FRAME_DURATION_US,
                };
                clock.advance_by_duration_us(broadcast_frame.duration_us, default_duration);
                Ok(SendResult::Continue)
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Subscriber lagged, dropped {} packets", n);
                let _ = ctx
                    .stats_delta_tx
                    .try_send(NodeStatsDelta { discarded: n, ..Default::default() });
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

/// Bundles the mutable loop state and immutable config that
/// [`MoqPeerNode::handle_broadcast_recv`] needs, replacing a 14-parameter
/// function signature with a single context reference.
struct SubscriberSendCtx<'a> {
    audio_track_producer: &'a mut Option<hang::container::OrderedProducer>,
    video_track_producer: &'a mut Option<hang::container::OrderedProducer>,
    packet_count: u64,
    frame_count: u64,
    /// Tracks whether the first audio frame has been sent so the initial
    /// MoQ group is opened independently of video frame ordering.
    audio_first_sent: bool,
    last_log: std::time::Instant,
    group_duration_ms: u64,
    audio_clock: MediaClock,
    video_clock: MediaClock,
    gap_histogram: opentelemetry::metrics::Histogram<f64>,
    metric_labels: [opentelemetry::KeyValue; 2],
    last_audio_ts_ms: Option<u64>,
    last_video_ts_ms: Option<u64>,
    stats_delta_tx: &'a mpsc::Sender<NodeStatsDelta>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn make_dynamic_output_pin_video_prefix() {
        let pin = make_dynamic_output_pin("video/hd");
        assert_eq!(pin.name, "video/hd");
        assert!(
            matches!(pin.produces_type, PacketType::EncodedVideo(_)),
            "video/ prefix should produce EncodedVideo"
        );
    }

    #[test]
    fn make_dynamic_output_pin_audio_prefix() {
        let pin = make_dynamic_output_pin("audio/data");
        assert_eq!(pin.name, "audio/data");
        assert!(
            matches!(pin.produces_type, PacketType::EncodedAudio(_)),
            "audio/ prefix should produce EncodedAudio"
        );
    }

    #[test]
    fn make_dynamic_output_pin_bare_name_defaults_to_audio() {
        let pin = make_dynamic_output_pin("some_track");
        assert_eq!(pin.name, "some_track");
        assert!(
            matches!(pin.produces_type, PacketType::EncodedAudio(_)),
            "bare name without video/ prefix should default to EncodedAudio"
        );
    }

    /// Regression: `AddedInputPin` previously used `..` to discard the channel,
    /// causing it to be immediately dropped and closing the sender side.
    /// Verify that `handle_pin_management` keeps the channel alive.
    #[tokio::test]
    async fn added_input_pin_channel_not_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (broadcast_tx, _broadcast_rx) = broadcast::channel::<BroadcastFrame>(16);

        let (stats_delta_tx, _stats_delta_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        let pin = InputPin {
            name: "audio/extra".to_string(),
            accepts_types: vec![],
            cardinality: PinCardinality::One,
        };
        let msg = PinManagementMessage::AddedInputPin { pin, channel: rx };
        let mut forwarder_handles = std::collections::HashMap::new();
        MoqPeerNode::handle_pin_management(
            msg,
            &dynamic_outputs,
            &broadcast_tx,
            &stats_delta_tx,
            &shutdown_tx,
            &mut forwarder_handles,
        );

        // If the channel was dropped, try_send would return a closed error.
        // A successful send (or full-buffer error) means the receiver is alive.
        assert!(!tx.is_closed(), "channel should remain open after AddedInputPin is handled");
    }

    /// Regression: dynamic pin names were double-prefixed (e.g. "audio/audio/data")
    /// because catalog rendition keys already include the prefix. Verify that
    /// `make_dynamic_output_pin` and `output_pins()` produce correct names.
    #[test]
    fn catalog_track_names_not_double_prefixed() {
        // Verify static output pins use track-named format
        let node = MoqPeerNode::new(MoqPeerConfig {
            gateway_path: "/moq".to_string(),
            input_broadcasts: vec!["input".to_string()],
            output_broadcast: "output".to_string(),
            allow_reconnect: false,
            output_group_duration_ms: 0,
            output_initial_delay_ms: 0,
            video_width: 640,
            video_height: 480,
        });
        let pins = node.output_pins();
        assert_eq!(pins[0].name, "audio/data");
        assert_eq!(pins[1].name, "video/data");
        assert!(!pins[0].name.starts_with("audio/audio/"), "audio pin must not be double-prefixed");
        assert!(!pins[1].name.starts_with("video/video/"), "video pin must not be double-prefixed");

        // Verify make_dynamic_output_pin preserves catalog track names as-is
        let audio_pin = make_dynamic_output_pin("audio/data");
        assert_eq!(audio_pin.name, "audio/data");
        assert!(matches!(audio_pin.produces_type, PacketType::EncodedAudio(_)));

        let video_pin = make_dynamic_output_pin("video/hd");
        assert_eq!(video_pin.name, "video/hd");
        assert!(matches!(video_pin.produces_type, PacketType::EncodedVideo(_)));

        // Verify broadcast-prefixed pin names are classified correctly
        let prefixed_video = make_dynamic_output_pin("screen-input/video/hd");
        assert_eq!(prefixed_video.name, "screen-input/video/hd");
        assert!(
            matches!(prefixed_video.produces_type, PacketType::EncodedVideo(_)),
            "Broadcast-prefixed video pin must be EncodedVideo, not EncodedAudio"
        );

        let prefixed_audio = make_dynamic_output_pin("cam-input/audio/data");
        assert_eq!(prefixed_audio.name, "cam-input/audio/data");
        assert!(
            matches!(prefixed_audio.produces_type, PacketType::EncodedAudio(_)),
            "Broadcast-prefixed audio pin must be EncodedAudio"
        );
    }

    // --- has_expected_tracks tests ---

    /// Helper: build a dummy track_handles map with the given track names.
    fn dummy_track_handles(
        names: &[&str],
    ) -> std::collections::HashMap<String, tokio::task::JoinHandle<Result<(), StreamKitError>>>
    {
        names.iter().map(|n| (n.to_string(), tokio::spawn(async { Ok(()) }))).collect()
    }

    #[tokio::test]
    async fn has_expected_tracks_primary_audio_and_video() {
        let handles = dummy_track_handles(&["audio/data", "video/hd"]);
        assert!(
            MoqPeerNode::has_expected_tracks(&handles, false),
            "Primary with both audio and video should be satisfied"
        );
    }

    #[tokio::test]
    async fn has_expected_tracks_primary_audio_only() {
        // Audio-only: should NOT be satisfied — caller uses grace timeout instead.
        let handles = dummy_track_handles(&["audio/data"]);
        assert!(
            !MoqPeerNode::has_expected_tracks(&handles, false),
            "Audio-only should not satisfy primary (grace timeout handles single-media)"
        );
    }

    #[tokio::test]
    async fn has_expected_tracks_primary_video_only() {
        // Video-only: should NOT be satisfied — caller uses grace timeout instead.
        let handles = dummy_track_handles(&["video/hd"]);
        assert!(
            !MoqPeerNode::has_expected_tracks(&handles, false),
            "Video-only should not satisfy primary (grace timeout handles single-media)"
        );
    }

    #[tokio::test]
    async fn has_expected_tracks_additional_any_track() {
        let handles = dummy_track_handles(&["video/hd"]);
        assert!(
            MoqPeerNode::has_expected_tracks(&handles, true),
            "Additional broadcast should be satisfied with any track"
        );
    }

    #[tokio::test]
    async fn has_expected_tracks_empty_handles() {
        let handles = dummy_track_handles(&[]);
        assert!(
            !MoqPeerNode::has_expected_tracks(&handles, false),
            "Empty handles should never be satisfied"
        );
        assert!(
            !MoqPeerNode::has_expected_tracks(&handles, true),
            "Empty handles should never be satisfied for additional either"
        );
    }
}
