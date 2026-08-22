// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

mod config;

pub use config::MoqPeerConfig;
use config::{
    infer_kind_from_packet, join_gateway_path, make_broadcast_frame, media_kind_for_packet_type,
    normalize_gateway_path, BidirectionalTaskConfig, BroadcastFrame, DynamicOutputs, FrameResult,
    MediaCodecConfig, MediaKind, MediaTypeState, NodeStatsDelta, PublisherEvent,
    PublisherReceiveLoopWithSlotConfig, SendResult, SubscriberMediaConfig, SubscriberSendCtx,
    TrackExit, TrackRouting,
};

use crate::transport::moq::constants::{
    audio_codec_from_catalog, catalog_audio_codec, catalog_video_codec, resolve_audio_codec,
    resolve_video_codec, video_codec_from_catalog,
};
use crate::transport::moq::discovered::{
    discovered_codec, record_discovered_codec, remove_discovered_codec, DiscoveredCodec,
    DiscoveredCodecs,
};
use crate::video::{AV1_CONTENT_TYPE, H264_CONTENT_TYPE, VP9_CONTENT_TYPE};
use async_trait::async_trait;
use std::collections::HashMap;
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

/// Initial timeout (seconds) for the first catalog update to arrive.
const CATALOG_INITIAL_TIMEOUT_SECS: u64 = 30;
/// Grace timeout (seconds) after the first tracks are discovered,
/// allowing time for a second media type (e.g. mic then camera)
/// without waiting the full initial timeout for genuinely
/// single-media pipelines.
const CATALOG_GRACE_TIMEOUT_SECS: u64 = 5;

/// Capacity for the broadcast channel (subscribers).
///
/// With audio+video multiplexed (~50 fps audio + 30 fps video = ~80 fps),
/// 256 entries give a slow subscriber roughly 3 seconds of buffer before
/// frames are dropped due to lagging. This is adequate for real-time
/// streaming; increase if subscribers are expected to be bursty-slow.
const SUBSCRIBER_BROADCAST_CAPACITY: usize = 256;

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
        let codec = self.config.video_codec.unwrap_or(VideoCodec::Vp9);
        let acodec = self.config.audio_codec.unwrap_or(AudioCodec::Opus);
        let codecs = MediaCodecConfig { video: codec, audio: acodec };
        vec![
            make_dynamic_output_pin("audio/data", codecs, None),
            make_dynamic_output_pin("video/data", codecs, None),
        ]
    }

    fn supports_dynamic_pins(&self) -> bool {
        true
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

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

        // Detect the upstream video codec so the subscriber catalog reflects
        // the actual encoding.  Priority order:
        // 1. Explicit `video_codec` config param (required for dynamic pipelines)
        // 2. Auto-detected from `input_types` (static pipelines)
        // 3. Default: VP9
        let video_codec = resolve_video_codec(self.config.video_codec, &context.input_types);

        // Subscriber-side audio codec: use `subscriber_audio_codec` config if
        // set, otherwise fall back to `audio_codec` / auto-detect from
        // `input_types`.  This allows transcoding pipelines (e.g. Opus in →
        // AAC out) to advertise the correct codec in the subscriber catalog
        // without changing the publisher output pin type.
        let subscriber_audio_codec = resolve_audio_codec(
            self.config.subscriber_audio_codec.or(self.config.audio_codec),
            &context.input_types,
        );

        // Publisher-side audio codec: used for dynamic output pins that carry
        // data FROM the publisher.  Must match `output_pins()` logic so that
        // runtime-created pins (e.g. for non-primary broadcasts) have the
        // correct type.
        let publisher_audio_codec =
            resolve_audio_codec(self.config.audio_codec, &context.input_types);

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
        // Per-pin codecs read from remote publishers' catalogs, so dynamically
        // created pins advertise the remote codec, not the local config.
        let discovered_codecs: DiscoveredCodecs = Arc::default();

        // Pin management channel for runtime pin creation
        let mut pin_mgmt_rx = context.pin_management_rx.take();

        // Track JoinHandles for dynamic input forwarder tasks so they can be
        // aborted promptly when the corresponding pin is removed.
        let mut forwarder_handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();

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
                            broadcast_rx,
                            shutdown_rx: shutdown_tx.subscribe(),
                            publisher_slot: publisher_slot.clone(),
                            publisher_events: publisher_events_tx.clone(),
                            subscriber_count: sub_count,
                            media: SubscriberMediaConfig {
                                has_video,
                                has_audio,
                                video_width: self.config.video_width,
                                video_height: self.config.video_height,
                                output_group_duration_ms: self.config.output_group_duration_ms,
                                output_initial_delay_ms: self.config.output_initial_delay_ms,
                                video_codec,
                                audio_codec: subscriber_audio_codec,
                            },
                            media_state_rx: media_state_rx.clone(),
                            routing: TrackRouting {
                                output_sender: context.output_sender.clone(),
                                stats_delta_tx: stats_delta_tx.clone(),
                                dynamic_outputs: dynamic_outputs.clone(),
                                discovered_codecs: discovered_codecs.clone(),
                                video_codec,
                            },
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
                        shutdown_tx.subscribe(),
                        publisher_events_tx.clone(),
                        TrackRouting {
                            output_sender: context.output_sender.clone(),
                            stats_delta_tx: stats_delta_tx.clone(),
                            dynamic_outputs: dynamic_outputs.clone(),
                            discovered_codecs: discovered_codecs.clone(),
                            video_codec,
                        },
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
                            video_codec,
                            audio_codec: subscriber_audio_codec,
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
                    Self::handle_pin_management(msg, &dynamic_outputs, &discovered_codecs, &subscriber_broadcast_tx, &stats_delta_tx, &shutdown_tx, &mut forwarder_handles, MediaCodecConfig { video: video_codec, audio: publisher_audio_codec });
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

/// Prefers the catalog-discovered codec (and media kind) for the pin. When
/// the catalog hasn't been seen yet, falls back to a name heuristic: names
/// containing a `video/` segment produce [`PacketType::EncodedVideo`] with the
/// local `codecs.video`; all others produce [`PacketType::EncodedAudio`] with
/// `codecs.audio`. This matches the convention in
/// `MoqPullNode::output_pins_for_tracks`.
///
/// The name heuristic handles both unprefixed names (`video/hd`) and
/// broadcast-prefixed names (`screen-input/video/hd`). It relies on the
/// naming convention enforced by [`MoqPeerNode::watch_catalog_and_process_inner`],
/// which constructs pin names as `{prefix}/{track_name}` where `track_name`
/// always begins with `audio/` or `video/`.  Broadcast prefixes are simple
/// identifiers (e.g. `screen-input`) so a false-positive match on `/video/`
/// in the prefix portion is not possible in practice.
fn make_dynamic_output_pin(
    name: &str,
    codecs: MediaCodecConfig,
    discovered: Option<DiscoveredCodec>,
) -> OutputPin {
    let is_video = match discovered {
        Some(DiscoveredCodec::Video(_)) => true,
        Some(DiscoveredCodec::Audio(_)) => false,
        None => name.starts_with("video/") || name.contains("/video/"),
    };
    let produces_type = if is_video {
        let codec = match discovered {
            Some(DiscoveredCodec::Video(c)) => c,
            _ => codecs.video,
        };
        PacketType::EncodedVideo(EncodedVideoFormat {
            codec,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        })
    } else {
        let codec = match discovered {
            Some(DiscoveredCodec::Audio(c)) => c,
            _ => codecs.audio,
        };
        PacketType::EncodedAudio(EncodedAudioFormat { codec, codec_private: None })
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
    // Pin lifecycle handling needs both routing maps plus codec config; bundling into a config struct is a future cleanup.
    #[allow(clippy::too_many_arguments)]
    fn handle_pin_management(
        msg: PinManagementMessage,
        dynamic_outputs: &DynamicOutputs,
        discovered_codecs: &DiscoveredCodecs,
        subscriber_broadcast_tx: &broadcast::Sender<BroadcastFrame>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        shutdown_tx: &broadcast::Sender<()>,
        forwarder_handles: &mut HashMap<String, tokio::task::JoinHandle<()>>,
        codecs: MediaCodecConfig,
    ) {
        match msg {
            PinManagementMessage::RequestAddOutputPin { suggested_name, response_tx } => {
                let pin_name = suggested_name.unwrap_or_else(|| "dynamic_out".to_string());
                // Prefer the codec the remote publisher's catalog advertises
                // for this pin over the local codec config — peers may
                // publish a different codec than this pipeline sends out.
                let discovered = discovered_codec(discovered_codecs, &pin_name);
                tracing::info!(
                    pin = %pin_name,
                    ?discovered,
                    "MoqPeerNode: creating dynamic output pin"
                );
                let pin = make_dynamic_output_pin(&pin_name, codecs, discovered);
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
            PinManagementMessage::AddedInputPin { pin, channel, hint_tx: _ } => {
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
                remove_discovered_codec(discovered_codecs, &pin_name);
            },
            // Type info for pre-existing pins; not used by this node.
            PinManagementMessage::InputTypeResolved { .. }
            | PinManagementMessage::OutputHintChannel { .. }
            | PinManagementMessage::AttachHintSender { .. } => {},
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
        forwarder_handles: &mut HashMap<String, tokio::task::JoinHandle<()>>,
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
    async fn start_publisher_task_with_permit(
        moq_connection: streamkit_core::moq_gateway::MoqConnection,
        permit: OwnedSemaphorePermit,
        input_broadcast: String,
        mut shutdown_rx: broadcast::Receiver<()>,
        publisher_events: mpsc::UnboundedSender<PublisherEvent>,
        routing: TrackRouting,
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
        let client_publish_origin = moq_net::Origin::random().produce();
        let receive_origin = client_publish_origin.consume();

        // Accept MoQ session (publisher only sends, no server publish needed)
        let session = request
            .with_subscriber(client_publish_origin)
            .ok()
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Failed to accept session: {e}")))?;

        let handle = tokio::spawn(async move {
            let _permit = permit;
            let _ = publisher_events.send(PublisherEvent::Connected { path: path.clone() });

            let result = Self::publisher_receive_loop(
                receive_origin,
                input_broadcast,
                &mut shutdown_rx,
                routing,
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
        let server_publish_origin = moq_net::Origin::random().produce();
        let send_origin = server_publish_origin.clone();

        let client_publish_origin = moq_net::Origin::random().produce();
        let receive_origin = client_publish_origin.consume();

        let session = request
            .with_publisher(server_publish_origin.consume())
            .with_subscriber(client_publish_origin)
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
            let subscriber_stats_delta_tx = config.routing.stats_delta_tx.clone();

            // Create per-broadcast OriginConsumer clones BEFORE moving
            // receive_origin into the primary loop.
            let extra_consumers: Vec<(String, moq_net::origin::Consumer)> =
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
                        publisher_slot: config.publisher_slot,
                        publisher_events: config.publisher_events,
                        publisher_path: path.clone(),
                        routing: config.routing.clone(),
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
                    let routing = config.routing.clone();
                    let mut shutdown = extra_shutdown_rx.resubscribe();

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
                            &mut shutdown,
                            &routing,
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
            // Errors are logged but intentionally NOT propagated: a secondary
            // broadcast failure (e.g. catalog timeout, network blip) should not
            // tear down the primary publish/subscribe session.  The primary
            // session already ran to completion via `tokio::join!` above.
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

        let result =
            Self::watch_catalog_and_process(&broadcast_consumer, shutdown_rx, &config.routing)
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
        subscribe: moq_net::origin::Consumer,
        broadcast_name: String,
        shutdown_rx: &mut broadcast::Receiver<()>,
        routing: TrackRouting,
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
        Self::watch_catalog_and_process(&broadcast_consumer, shutdown_rx, &routing).await
    }

    /// Wait for the publisher to announce the expected broadcast
    async fn wait_for_broadcast_announcement(
        subscribe: moq_net::origin::Consumer,
        broadcast_name: &str,
        shutdown_rx: &mut broadcast::Receiver<()>,
    ) -> Result<Option<moq_net::broadcast::Consumer>, StreamKitError> {
        let mut announced = subscribe.announced();
        loop {
            tokio::select! {
                announcement = announced.next() => {
                    match announcement {
                        Some(moq_net::announce::Update { path, broadcast: Some(consumer) }) => {
                            tracing::info!("Publisher announced broadcast: {}", path.as_str());
                            if path.as_str() == broadcast_name {
                                return Ok(Some(consumer));
                            }
                        }
                        Some(moq_net::announce::Update { path, broadcast: None }) => {
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
        track_handles: &HashMap<String, tokio::task::JoinHandle<Result<(), StreamKitError>>>,
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
        broadcast_consumer: &moq_net::broadcast::Consumer,
        shutdown_rx: &mut broadcast::Receiver<()>,
        routing: &TrackRouting,
    ) -> Result<(), StreamKitError> {
        Self::watch_catalog_and_process_inner(
            broadcast_consumer,
            shutdown_rx,
            routing,
            None, // no pin prefix for single-broadcast mode
        )
        .await
    }

    /// Subscribe to newly discovered tracks from a catalog update.
    ///
    /// For each audio and video rendition in the catalog that isn't already
    /// being handled, spawns a track processor task with the appropriate
    /// output pin name (optionally prefixed for multi-broadcast mode).
    ///
    /// **Important:** `track_handles` is scoped to a single broadcast.
    /// Each broadcast must use its own map — sharing a map across broadcasts
    /// would cause track-name collisions (e.g. two `"video/hd"` entries).
    fn subscribe_catalog_tracks(
        catalog: &hang::catalog::Catalog,
        broadcast_consumer: &moq_net::broadcast::Consumer,
        shutdown_rx: &broadcast::Receiver<()>,
        routing: &TrackRouting,
        pin_prefix: Option<&str>,
        track_handles: &mut HashMap<String, tokio::task::JoinHandle<Result<(), StreamKitError>>>,
    ) {
        // Subscribe to all audio tracks not yet being handled. Like video
        // below, each track records the codec its catalog rendition advertises
        // so dynamically created pins are typed with the remote codec.
        for (track_name, rendition) in &catalog.audio.renditions {
            if !track_handles.contains_key(track_name) {
                let track_codec = audio_codec_from_catalog(&rendition.codec);
                tracing::info!(
                    track = %track_name,
                    catalog_codec = ?rendition.codec,
                    resolved_codec = ?track_codec,
                    "Found audio track in catalog"
                );
                let output_pin = pin_prefix
                    .map_or_else(|| track_name.clone(), |prefix| format!("{prefix}/{track_name}"));
                if let Some(codec) = track_codec {
                    record_discovered_codec(
                        &routing.discovered_codecs,
                        &output_pin,
                        DiscoveredCodec::Audio(codec),
                    );
                }
                track_handles.insert(
                    track_name.clone(),
                    Self::spawn_track_processor_with_pin(
                        broadcast_consumer,
                        track_name,
                        false,
                        shutdown_rx,
                        routing,
                        &output_pin,
                        routing.video_codec,
                    ),
                );
            }
        }

        // Subscribe to all video tracks not yet being handled. Each track is
        // labeled with the codec its catalog rendition advertises — the local
        // `video_codec` config describes this pipeline's outgoing media and may
        // differ from what the remote peer publishes (issue #529).
        for (track_name, rendition) in &catalog.video.renditions {
            if !track_handles.contains_key(track_name) {
                let track_codec = video_codec_from_catalog(&rendition.codec);
                tracing::info!(
                    track = %track_name,
                    catalog_codec = ?rendition.codec,
                    resolved_codec = ?track_codec,
                    fallback_codec = ?routing.video_codec,
                    "Found video track in catalog"
                );
                let track_codec = track_codec.unwrap_or_else(|| {
                    tracing::warn!(
                        track = %track_name,
                        catalog_codec = ?rendition.codec,
                        "Unsupported catalog video codec; falling back to local video_codec config"
                    );
                    routing.video_codec
                });
                let output_pin = pin_prefix
                    .map_or_else(|| track_name.clone(), |prefix| format!("{prefix}/{track_name}"));
                record_discovered_codec(
                    &routing.discovered_codecs,
                    &output_pin,
                    DiscoveredCodec::Video(track_codec),
                );
                track_handles.insert(
                    track_name.clone(),
                    Self::spawn_track_processor_with_pin(
                        broadcast_consumer,
                        track_name,
                        true,
                        shutdown_rx,
                        routing,
                        &output_pin,
                        track_codec,
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
        broadcast_consumer: &moq_net::broadcast::Consumer,
        shutdown_rx: &mut broadcast::Receiver<()>,
        routing: &TrackRouting,
        pin_prefix: Option<&str>,
    ) -> Result<(), StreamKitError> {
        let catalog_track =
            crate::transport::moq::subscribe_catalog(broadcast_consumer).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to subscribe to catalog track: {e}"))
            })?;
        let mut catalog_consumer =
            crate::transport::moq::catalog_consumer::CatalogConsumer::new(catalog_track);

        let mut track_handles: HashMap<
            String,
            tokio::task::JoinHandle<Result<(), StreamKitError>>,
        > = HashMap::new();

        let mut tracks_discovered = false;

        // Monitor the catalog for new tracks, subscribing to each as it appears
        loop {
            let timeout_secs = if tracks_discovered {
                CATALOG_GRACE_TIMEOUT_SECS
            } else {
                CATALOG_INITIAL_TIMEOUT_SECS
            };
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
                        shutdown_rx,
                        routing,
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
    ///
    /// `video_codec` is the per-track catalog-resolved codec (which may differ
    /// from `routing.video_codec`, the local fallback) — it labels the frame
    /// `content_type` for video tracks.
    fn spawn_track_processor_with_pin(
        broadcast_consumer: &moq_net::broadcast::Consumer,
        track_name: &str,
        is_video: bool,
        shutdown_rx: &broadcast::Receiver<()>,
        routing: &TrackRouting,
        output_pin_name: &str,
        video_codec: VideoCodec,
    ) -> tokio::task::JoinHandle<Result<(), StreamKitError>> {
        const MAX_RESUBSCRIBE_ATTEMPTS: u32 = 10;
        const RESUBSCRIBE_INITIAL_BACKOFF: Duration = Duration::from_millis(100);

        let broadcast = broadcast_consumer.clone();
        let track =
            crate::transport::moq::TrackRef { name: track_name.to_string(), priority: 2 };
        let sender = routing.output_sender.clone();
        let mut task_shutdown = shutdown_rx.resubscribe();
        let stats = routing.stats_delta_tx.clone();
        let output_pin = output_pin_name.to_string();
        let dyn_outputs = routing.dynamic_outputs.clone();

        tokio::spawn(async move {
            tracing::info!(output_pin = %output_pin, track = %track.name, "Track processor task started");

            let mut attempt: u32 = 0;
            loop {
                let consumer =
                    crate::transport::moq::subscribe_track(&broadcast, &track.name, track.priority)
                        .await
                        .map_err(|e| {
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
                    video_codec,
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
        track_handles: HashMap<String, tokio::task::JoinHandle<Result<(), StreamKitError>>>,
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
        mut track_consumer: moq_net::track::Subscriber,
        mut output_sender: streamkit_core::OutputSender,
        output_pin: &str,
        is_video: bool,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: &DynamicOutputs,
        video_codec: VideoCodec,
    ) -> TrackExit {
        let mut frame_count = 0u64;
        let mut last_log = std::time::Instant::now();
        let mut current_group: Option<moq_net::group::Consumer> = None;
        // Base timestamp for normalization — the first MoQ timestamp on this
        // track is subtracted from all subsequent timestamps so that every
        // track's timeline starts near 0.  This prevents overflow in
        // downstream containers (WebM timecodes are limited) and lets the
        // muxer's rebase offset align tracks by arrival time.
        let mut base_timestamp_us: Option<u64> = None;
        let mut first_frame_logged = false;
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
                    Err(moq_net::Error::Cancel) => return TrackExit::Cancelled,
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
                    &mut base_timestamp_us,
                    &mut first_frame_logged,
                    shutdown_rx,
                    stats_delta_tx,
                    keyframe,
                    dynamic_outputs,
                    video_codec,
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
    /// Surfaces the raw [`moq_net::Error`] so the caller can distinguish
    /// [`moq_net::Error::Cancel`] (publisher dropped the track producer —
    /// retryable) from other failures. The `tracing::warn!` here will still
    /// fire for cancellations; that's intentional since they're unexpected
    /// in steady state even if we recover.
    async fn get_next_group(
        track_consumer: &mut moq_net::track::Subscriber,
        shutdown_rx: &mut broadcast::Receiver<()>,
        output_pin: &str,
    ) -> Result<Option<moq_net::group::Consumer>, moq_net::Error> {
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
                match output_sender.send(output_pin, packet).await {
                    Ok(()) => true,
                    Err(streamkit_core::OutputSendError::PinNotFound { .. }) => {
                        // The pin doesn't exist yet — the engine may still be
                        // wiring up a dynamic output pin for this track.  Drop
                        // the packet but keep the track processor alive so it
                        // can deliver subsequent frames once the pin appears.
                        tracing::debug!(
                            output_pin,
                            "Output pin not yet available, dropping packet"
                        );
                        true
                    },
                    Err(_) => false,
                }
            },
        }
    }

    /// Process a single frame from the current group.
    ///
    /// `is_keyframe` indicates whether this is the first frame of a new MoQ
    /// group, which in the hang protocol corresponds to a keyframe boundary.
    #[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
    async fn process_frame_from_group(
        group: &mut moq_net::group::Consumer,
        output_sender: &mut streamkit_core::OutputSender,
        output_pin: &str,
        is_video: bool,
        frame_count: &mut u64,
        last_log: &mut std::time::Instant,
        base_timestamp_us: &mut Option<u64>,
        first_frame_logged: &mut bool,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        is_keyframe: bool,
        dynamic_outputs: &DynamicOutputs,
        video_codec: VideoCodec,
    ) -> Result<FrameResult, StreamKitError> {
        tokio::select! {
            biased;
            frame_result = group.read_frame() => {
                match frame_result {
                    Ok(Some(net_frame)) => {
                        *frame_count += 1;

                        if last_log.elapsed() > Duration::from_secs(1) {
                            tracing::debug!("Publisher: received {} frames/sec", *frame_count);
                            *frame_count = 0;
                            *last_log = std::time::Instant::now();
                        }

                        // Decode the hang container frame (varint-encoded microsecond
                        // timestamp prefix + payload) and propagate the timestamp as
                        // PacketMetadata so downstream nodes have timing.
                        let frame = match hang::container::Frame::decode(net_frame.payload) {
                            Ok(frame) => frame,
                            Err(e) => {
                                tracing::warn!("Failed to decode frame timestamp: {e}");
                                let _ = stats_delta_tx
                                    .try_send(NodeStatsDelta { received: 1, discarded: 1, ..Default::default() });
                                return Ok(FrameResult::Continue);
                            },
                        };
                        #[allow(clippy::cast_possible_truncation)] // MoQ timestamps fit in u64
                        let raw_timestamp_us = frame.timestamp.as_micros() as u64;

                        // Normalize: subtract the first timestamp so the track
                        // starts near 0.  This avoids WebM timecode overflow
                        // (video timestamps from browsers can be 41+ days).
                        let base = *base_timestamp_us.get_or_insert(raw_timestamp_us);
                        let timestamp_us = raw_timestamp_us.saturating_sub(base);

                        if !*first_frame_logged {
                            *first_frame_logged = true;
                            tracing::info!(
                                "MoQ first frame: video={is_video} raw_ts={raw_timestamp_us}us \
                                 normalized_ts={timestamp_us}us base={base}us \
                                 keyframe={is_keyframe} size={}B",
                                frame.payload.len()
                            );
                        }

                        let data = frame.payload;
                        let content_type = if is_video {
                            Some(std::borrow::Cow::Borrowed(match video_codec {
                                VideoCodec::Av1 => AV1_CONTENT_TYPE,
                                VideoCodec::H264 => H264_CONTENT_TYPE,
                                _ => VP9_CONTENT_TYPE,
                            }))
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
        let server_publish_origin = moq_net::Origin::random().produce();
        let send_origin = server_publish_origin.clone();

        // Accept MoQ session (subscriber only receives, no client publish needed)
        let session = request
            .with_publisher(server_publish_origin.consume())
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
        publish: moq_net::origin::Producer,
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
            media.audio_codec,
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
        publish: &moq_net::origin::Producer,
        broadcast_name: &str,
        media: &SubscriberMediaConfig,
    ) -> Result<
        (
            moq_net::broadcast::Producer,
            Option<crate::transport::moq::ordered_producer::OrderedProducer>,
            Option<crate::transport::moq::ordered_producer::OrderedProducer>,
            moq_net::track::Producer,
        ),
        StreamKitError,
    > {
        // Create broadcast
        let mut broadcast_producer = publish
            .create_broadcast(broadcast_name, moq_net::broadcast::Route::announced())
            .map_err(|e| {
                StreamKitError::Runtime(format!(
                    "Failed to create broadcast '{broadcast_name}': {e}"
                ))
            })?;

        // Create audio track (if audio input connected)
        let audio_track = if media.has_audio {
            let track =
                crate::transport::moq::TrackRef { name: "audio/data".to_string(), priority: 80 };
            let producer = crate::transport::moq::create_media_track(
                &mut broadcast_producer,
                &track.name,
                track.priority,
            )
            .map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create audio track: {e}"))
            })?;
            Some((track, crate::transport::moq::ordered_producer::OrderedProducer::from(producer)))
        } else {
            None
        };

        // Create video track (if video input connected)
        let video_track = if media.has_video {
            let track =
                crate::transport::moq::TrackRef { name: "video/data".to_string(), priority: 60 };
            let producer = crate::transport::moq::create_media_track(
                &mut broadcast_producer,
                &track.name,
                track.priority,
            )
            .map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create video track: {e}"))
            })?;
            Some((track, crate::transport::moq::ordered_producer::OrderedProducer::from(producer)))
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
            media.video_codec,
            media.audio_codec,
        )?;

        Ok((
            broadcast_producer,
            audio_track.map(|(_, p)| p),
            video_track.map(|(_, p)| p),
            catalog_producer,
        ))
    }

    /// Create and publish the catalog with audio and/or video track info
    #[allow(clippy::too_many_arguments)]
    fn create_and_publish_catalog(
        broadcast_producer: &mut moq_net::broadcast::Producer,
        audio_track: Option<&crate::transport::moq::TrackRef>,
        video_track: Option<&crate::transport::moq::TrackRef>,
        video_width: u32,
        video_height: u32,
        video_codec: VideoCodec,
        audio_codec: AudioCodec,
    ) -> Result<moq_net::track::Producer, StreamKitError> {
        let mut audio_renditions = std::collections::BTreeMap::new();
        if let Some(audio_track) = audio_track {
            audio_renditions.insert(audio_track.name.clone(), {
                // AAC-LC encoder always outputs stereo (upmixing mono
                // input), so advertise 2 channels when the subscriber
                // codec is AAC.  Opus uses mono by default.
                let channel_count = match audio_codec {
                    AudioCodec::Aac => 2,
                    _ => 1,
                };
                let mut cfg = hang::catalog::AudioConfig::new(
                    catalog_audio_codec(audio_codec),
                    48000,
                    channel_count,
                );
                cfg.bitrate = Some(64_000);
                cfg
            });
        }

        let mut video_renditions = std::collections::BTreeMap::new();
        if let Some(video_track) = video_track {
            video_renditions.insert(video_track.name.clone(), {
                let mut cfg = hang::catalog::VideoConfig::new(catalog_video_codec(video_codec));
                cfg.coded_width = Some(video_width);
                cfg.coded_height = Some(video_height);
                cfg.framerate = Some(30.0);
                cfg.optimize_for_latency = Some(true);
                cfg
            });
        }

        let mut catalog = hang::catalog::Catalog::default();
        catalog.audio.renditions = audio_renditions;
        catalog.video.renditions = video_renditions;

        let mut catalog_producer = crate::transport::moq::create_catalog_track(broadcast_producer)
            .map_err(|e| StreamKitError::Runtime(format!("Failed to create catalog track: {e}")))?;
        let catalog_json = catalog
            .to_json()
            .map_err(|e| StreamKitError::Runtime(format!("Failed to serialize catalog: {e}")))?;
        crate::transport::moq::write_catalog_json(&mut catalog_producer, catalog_json)
            .map_err(|e| StreamKitError::Runtime(format!("Failed to write catalog frame: {e}")))?;

        Ok(catalog_producer)
    }

    /// Run the main send loop, forwarding packets to the subscriber
    #[allow(clippy::too_many_arguments)]
    async fn run_subscriber_send_loop(
        audio_track_producer: &mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
        video_track_producer: &mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
        mut broadcast_rx: broadcast::Receiver<BroadcastFrame>,
        shutdown_rx: &mut broadcast::Receiver<()>,
        output_group_duration_ms: u64,
        output_initial_delay_ms: u64,
        node_id: String,
        broadcast_name: String,
        stats_delta_tx: &mpsc::Sender<NodeStatsDelta>,
        audio_codec: AudioCodec,
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
            audio_codec,
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
                let default_duration = match broadcast_frame.kind {
                    MediaKind::Audio => ctx.audio_codec.default_frame_duration_us(),
                    MediaKind::Video => crate::video::DEFAULT_VIDEO_FRAME_DURATION_US,
                };
                let timestamp =
                    hang::container::Timestamp::from_millis(timestamp_ms).map_err(|_| {
                        StreamKitError::Runtime("MoQ frame timestamp overflow".to_string())
                    })?;
                let frame = hang::container::Frame { timestamp, payload: broadcast_frame.data };

                match broadcast_frame.kind {
                    MediaKind::Video => {
                        // The OrderedProducer guarantees a group never opens on a
                        // delta (dropping leading deltas), so a late-joining
                        // subscriber always starts decoding on a keyframe.
                        match track_producer.write_video(&frame, broadcast_frame.keyframe) {
                            Ok(crate::transport::moq::ordered_producer::VideoWrite::Written) => {},
                            Ok(crate::transport::moq::ordered_producer::VideoWrite::DroppedLeadingDelta) => {
                                // Keep the video clock moving for the dropped frame
                                // so the kept keyframe stays time-aligned with the
                                // (ungated) audio track on this accumulation clock.
                                clock.advance_by_duration_us(broadcast_frame.duration_us, default_duration);
                                let _ = ctx
                                    .stats_delta_tx
                                    .try_send(NodeStatsDelta { discarded: 1, ..Default::default() });
                                return Ok(SendResult::Continue);
                            },
                            Err(e) => {
                                tracing::warn!("Failed to write MoQ video frame to subscriber: {e}");
                                let _ = ctx
                                    .stats_delta_tx
                                    .try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                                return Ok(SendResult::Stop);
                            },
                        }
                    },
                    MediaKind::Audio => {
                        let first = !ctx.audio_first_sent;
                        ctx.audio_first_sent = true;
                        let keyframe = first || clock.is_group_boundary_ms(ctx.group_duration_ms);
                        if keyframe {
                            if let Err(e) = track_producer.keyframe() {
                                tracing::warn!("Failed to signal audio group boundary: {e}");
                                let _ = ctx
                                    .stats_delta_tx
                                    .try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                                return Ok(SendResult::Stop);
                            }
                        }
                        if let Err(e) = track_producer.write(&frame) {
                            tracing::warn!("Failed to write MoQ audio frame to subscriber: {e}");
                            let _ = ctx
                                .stats_delta_tx
                                .try_send(NodeStatsDelta { errored: 1, ..Default::default() });
                            return Ok(SendResult::Stop);
                        }
                    },
                }

                if let Some(prev) = *last_ts_ms {
                    let gap = timestamp_ms.saturating_sub(prev);
                    ctx.gap_histogram.record(gap as f64, &ctx.metric_labels);
                }
                *last_ts_ms = Some(timestamp_ms);
                clock.advance_by_duration_us(broadcast_frame.duration_us, default_duration);
                ctx.frame_count += 1;
                Ok(SendResult::Continue)
            },
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!("Subscriber lagged, dropped {} packets", n);
                // No video re-gating here: the current group already opened on a
                // keyframe and stays open across the lag, so a subscriber joining
                // afterwards still receives a keyframe-led group. Re-gating on a
                // bare lag count (audio and video share this channel) would freeze
                // video for up to a full GOP on an audio-only lag.
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn make_dynamic_output_pin_video_prefix() {
        let pin = make_dynamic_output_pin(
            "video/hd",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(pin.name, "video/hd");
        assert!(
            matches!(pin.produces_type, PacketType::EncodedVideo(_)),
            "video/ prefix should produce EncodedVideo"
        );
    }

    #[test]
    fn make_dynamic_output_pin_audio_prefix() {
        let pin = make_dynamic_output_pin(
            "audio/data",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(pin.name, "audio/data");
        assert!(
            matches!(pin.produces_type, PacketType::EncodedAudio(_)),
            "audio/ prefix should produce EncodedAudio"
        );
    }

    #[test]
    fn make_dynamic_output_pin_bare_name_defaults_to_audio() {
        let pin = make_dynamic_output_pin(
            "some_track",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(pin.name, "some_track");
        assert!(
            matches!(pin.produces_type, PacketType::EncodedAudio(_)),
            "bare name without video/ prefix should default to EncodedAudio"
        );
    }

    /// Regression: `make_dynamic_output_pin` hardcoded VP9 for all video pins,
    /// causing type mismatches when `video_codec: av1` was configured.
    #[test]
    fn make_dynamic_output_pin_video_uses_av1_codec() {
        let pin = make_dynamic_output_pin(
            "video/hd",
            MediaCodecConfig { video: VideoCodec::Av1, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(pin.name, "video/hd");
        match &pin.produces_type {
            PacketType::EncodedVideo(fmt) => assert_eq!(
                fmt.codec,
                VideoCodec::Av1,
                "video pin should use the supplied AV1 codec, not default to VP9"
            ),
            other => panic!("expected EncodedVideo, got {other:?}"),
        }
    }

    #[test]
    fn make_dynamic_output_pin_video_uses_vp9_codec() {
        let pin = make_dynamic_output_pin(
            "video/hd",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        match &pin.produces_type {
            PacketType::EncodedVideo(fmt) => assert_eq!(fmt.codec, VideoCodec::Vp9),
            other => panic!("expected EncodedVideo, got {other:?}"),
        }
    }

    /// Broadcast-prefixed name with AV1 codec: the `/video/` infix should be
    /// detected and the supplied codec should be threaded through.
    #[test]
    fn make_dynamic_output_pin_broadcast_prefix_av1() {
        let pin = make_dynamic_output_pin(
            "screen-input/video/hd",
            MediaCodecConfig { video: VideoCodec::Av1, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(pin.name, "screen-input/video/hd");
        match &pin.produces_type {
            PacketType::EncodedVideo(fmt) => assert_eq!(
                fmt.codec,
                VideoCodec::Av1,
                "broadcast-prefixed video pin should use AV1 codec"
            ),
            other => panic!("expected EncodedVideo, got {other:?}"),
        }
    }

    #[test]
    fn make_dynamic_output_pin_audio_uses_aac_codec() {
        let pin = make_dynamic_output_pin(
            "audio/data",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Aac },
            None,
        );
        match &pin.produces_type {
            PacketType::EncodedAudio(fmt) => assert_eq!(
                fmt.codec,
                AudioCodec::Aac,
                "audio pin should use the supplied AAC codec, not default to Opus"
            ),
            other => panic!("expected EncodedAudio, got {other:?}"),
        }
    }

    /// Regression: `output_pins()` should respect the configured `video_codec`
    /// so that the engine's type validation passes for AV1 pipelines.
    #[test]
    fn output_pins_respects_video_codec_config() {
        let node = MoqPeerNode::new(MoqPeerConfig {
            video_codec: Some(VideoCodec::Av1),
            ..MoqPeerConfig::default()
        });
        let pins = node.output_pins();
        let video_pin = pins.iter().find(|p| p.name == "video/data").unwrap();
        match &video_pin.produces_type {
            PacketType::EncodedVideo(fmt) => assert_eq!(
                fmt.codec,
                VideoCodec::Av1,
                "output_pins() should use AV1 when video_codec config is 'av1'"
            ),
            other => panic!("expected EncodedVideo, got {other:?}"),
        }
    }

    /// Regression: `AddedInputPin` previously used `..` to discard the channel,
    /// causing it to be immediately dropped and closing the sender side.
    /// Verify that `handle_pin_management` keeps the channel alive.
    #[tokio::test]
    async fn added_input_pin_channel_not_dropped() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Packet>(4);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (broadcast_tx, _broadcast_rx) = broadcast::channel::<BroadcastFrame>(16);

        let (stats_delta_tx, _stats_delta_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _) = broadcast::channel::<()>(1);

        let pin = InputPin {
            name: "audio/extra".to_string(),
            accepts_types: vec![],
            cardinality: PinCardinality::One,
        };
        let msg = PinManagementMessage::AddedInputPin { pin, channel: rx, hint_tx: None };
        let mut forwarder_handles = HashMap::new();
        MoqPeerNode::handle_pin_management(
            msg,
            &dynamic_outputs,
            &discovered_codecs,
            &broadcast_tx,
            &stats_delta_tx,
            &shutdown_tx,
            &mut forwarder_handles,
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
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
            video_codec: None,
            audio_codec: None,
            subscriber_audio_codec: None,
        });
        let pins = node.output_pins();
        assert_eq!(pins[0].name, "audio/data");
        assert_eq!(pins[1].name, "video/data");
        assert!(!pins[0].name.starts_with("audio/audio/"), "audio pin must not be double-prefixed");
        assert!(!pins[1].name.starts_with("video/video/"), "video pin must not be double-prefixed");

        // Verify make_dynamic_output_pin preserves catalog track names as-is
        let audio_pin = make_dynamic_output_pin(
            "audio/data",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(audio_pin.name, "audio/data");
        assert!(matches!(audio_pin.produces_type, PacketType::EncodedAudio(_)));

        let video_pin = make_dynamic_output_pin(
            "video/hd",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(video_pin.name, "video/hd");
        assert!(matches!(video_pin.produces_type, PacketType::EncodedVideo(_)));

        // Verify broadcast-prefixed pin names are classified correctly
        let prefixed_video = make_dynamic_output_pin(
            "screen-input/video/hd",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(prefixed_video.name, "screen-input/video/hd");
        assert!(
            matches!(prefixed_video.produces_type, PacketType::EncodedVideo(_)),
            "Broadcast-prefixed video pin must be EncodedVideo, not EncodedAudio"
        );

        let prefixed_audio = make_dynamic_output_pin(
            "cam-input/audio/data",
            MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus },
            None,
        );
        assert_eq!(prefixed_audio.name, "cam-input/audio/data");
        assert!(
            matches!(prefixed_audio.produces_type, PacketType::EncodedAudio(_)),
            "Broadcast-prefixed audio pin must be EncodedAudio"
        );
    }

    fn test_routing(
        output_sender: streamkit_core::OutputSender,
        stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
        dynamic_outputs: DynamicOutputs,
    ) -> TrackRouting {
        TrackRouting {
            output_sender,
            stats_delta_tx,
            dynamic_outputs,
            discovered_codecs: Arc::default(),
            video_codec: VideoCodec::Vp9,
        }
    }

    fn dummy_track_handles(
        names: &[&str],
    ) -> HashMap<String, tokio::task::JoinHandle<Result<(), StreamKitError>>> {
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

    /// Verify that `video_codec` config is correctly deserialized from JSON
    /// and that the default (None) falls through.
    #[test]
    fn video_codec_config_deserialization() {
        // Default: video_codec is None
        let default: MoqPeerConfig = serde_json::from_str("{}").unwrap();
        assert!(default.video_codec.is_none());

        // Explicit av1
        let av1: MoqPeerConfig = serde_json::from_str(r#"{"video_codec": "av1"}"#).unwrap();
        assert_eq!(av1.video_codec, Some(VideoCodec::Av1));

        // Explicit vp9
        let vp9: MoqPeerConfig = serde_json::from_str(r#"{"video_codec": "vp9"}"#).unwrap();
        assert_eq!(vp9.video_codec, Some(VideoCodec::Vp9));
    }

    // These tests drive the publisher and subscriber loops in-process using
    // `moq_net::Origin::random().produce()` producer/consumer pairs — no
    // network or relay required. Frames carry a hang varint timestamp prefix,
    // matching the wire format the loops decode.

    fn container_frame(timestamp_us: u64, data: &[u8]) -> hang::container::Frame {
        hang::container::Frame {
            timestamp: hang::container::Timestamp::from_micros(timestamp_us).unwrap(),
            payload: bytes::Bytes::copy_from_slice(data),
        }
    }

    fn write_group(producer: &mut moq_net::track::Producer, frames: &[(u64, &[u8])]) {
        let mut group = producer.append_group().unwrap();
        for (ts, data) in frames {
            container_frame(*ts, data).write_to(&mut group).unwrap();
        }
        group.finish().unwrap();
    }

    fn drain_stats(rx: &mut mpsc::Receiver<NodeStatsDelta>) -> NodeStatsDelta {
        let mut total = NodeStatsDelta::default();
        while let Ok(d) = rx.try_recv() {
            total.received += d.received;
            total.sent += d.sent;
            total.discarded += d.discarded;
            total.errored += d.errored;
        }
        total
    }

    fn sub_media(has_audio: bool, has_video: bool) -> SubscriberMediaConfig {
        SubscriberMediaConfig {
            has_video,
            has_audio,
            video_width: 640,
            video_height: 480,
            output_group_duration_ms: 40,
            output_initial_delay_ms: 0,
            video_codec: VideoCodec::Vp9,
            audio_codec: AudioCodec::Opus,
        }
    }

    fn audio_frame(data: &'static [u8]) -> BroadcastFrame {
        BroadcastFrame {
            data: bytes::Bytes::from_static(data),
            duration_us: Some(20_000),
            kind: MediaKind::Audio,
            keyframe: false,
        }
    }

    fn video_frame(data: &'static [u8], keyframe: bool) -> BroadcastFrame {
        BroadcastFrame {
            data: bytes::Bytes::from_static(data),
            duration_us: None,
            kind: MediaKind::Video,
            keyframe,
        }
    }

    /// Drive `run_subscriber_send_loop` with the fixed test parameters
    /// (group duration 40ms, no initial delay, Opus), returning the frame count.
    async fn drive_send_loop(
        audio: &mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
        video: &mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
        rx: broadcast::Receiver<BroadcastFrame>,
        shutdown_rx: &mut broadcast::Receiver<()>,
        stats_tx: &mpsc::Sender<NodeStatsDelta>,
    ) -> u64 {
        MoqPeerNode::run_subscriber_send_loop(
            audio,
            video,
            rx,
            shutdown_rx,
            40,
            0,
            "node".to_string(),
            "output".to_string(),
            stats_tx,
            AudioCodec::Opus,
        )
        .await
        .unwrap()
    }

    /// Build a `SubscriberSendCtx` mirroring the production initializer in
    /// `run_subscriber_send_loop` (including the `output_group_duration_ms.max(1)`
    /// clamp) so a field change can't silently drift the two apart. Used by
    /// tests that drive `handle_broadcast_recv` directly.
    fn make_test_ctx<'a>(
        audio: &'a mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
        video: &'a mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
        stats_tx: &'a mpsc::Sender<NodeStatsDelta>,
        output_group_duration_ms: u64,
    ) -> SubscriberSendCtx<'a> {
        let gap_histogram =
            opentelemetry::global::meter("test").f64_histogram("test.inter_frame_ms").build();
        SubscriberSendCtx {
            audio_track_producer: audio,
            video_track_producer: video,
            packet_count: 0,
            frame_count: 0,
            audio_first_sent: false,
            last_log: std::time::Instant::now(),
            group_duration_ms: output_group_duration_ms.max(1),
            audio_clock: MediaClock::new(0),
            video_clock: MediaClock::new(0),
            gap_histogram,
            metric_labels: [
                opentelemetry::KeyValue::new("node_id", "test"),
                opentelemetry::KeyValue::new("broadcast", "output"),
            ],
            last_audio_ts_ms: None,
            last_video_ts_ms: None,
            stats_delta_tx: stats_tx,
            audio_codec: AudioCodec::Opus,
        }
    }

    /// Read the next MoQ group off `sub` and return its frame payloads in
    /// order, so tests can assert wire-level group boundaries (a group must
    /// open on a keyframe) rather than internal producer state.
    async fn read_group_payloads(sub: &mut moq_net::track::Subscriber) -> Vec<Vec<u8>> {
        let mut group = tokio::time::timeout(Duration::from_secs(2), sub.next_group())
            .await
            .expect("next_group should not hang")
            .expect("next_group should succeed")
            .expect("a group should be available");
        let mut payloads = Vec::new();
        while let Some(frame) = tokio::time::timeout(Duration::from_secs(2), group.read_frame())
            .await
            .expect("read_frame should not hang")
            .expect("read_frame should succeed")
        {
            payloads.push(hang::container::Frame::decode(frame.payload).unwrap().payload.to_vec());
        }
        payloads
    }

    struct SingleTrackFixture {
        _origin: moq_net::origin::Producer,
        _broadcast: moq_net::broadcast::Producer,
        producer: moq_net::track::Producer,
        consumer: moq_net::track::Subscriber,
    }

    async fn make_single_track(track_name: &str) -> SingleTrackFixture {
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("input", moq_net::broadcast::Route::announced())
            .unwrap();
        let producer =
            crate::transport::moq::create_media_track(&mut broadcast, track_name, 2).unwrap();
        let bc = origin.consume().announced_broadcast("input").await.unwrap();
        let consumer = crate::transport::moq::subscribe_track(&bc, track_name, 2).await.unwrap();
        SingleTrackFixture { _origin: origin, _broadcast: broadcast, producer, consumer }
    }

    struct PublisherBroadcastFixture {
        _origin: moq_net::origin::Producer,
        _broadcast: moq_net::broadcast::Producer,
        _catalog: moq_net::track::Producer,
        audio: moq_net::track::Producer,
        video: moq_net::track::Producer,
        consumer: moq_net::origin::Consumer,
    }

    fn make_publisher_broadcast() -> PublisherBroadcastFixture {
        let origin = moq_net::Origin::random().produce();
        let consumer = origin.consume();
        let mut broadcast = origin
            .create_broadcast("input", moq_net::broadcast::Route::announced())
            .unwrap();

        let audio_track =
            crate::transport::moq::TrackRef { name: "audio/data".to_string(), priority: 2 };
        let video_track =
            crate::transport::moq::TrackRef { name: "video/data".to_string(), priority: 2 };
        let audio =
            crate::transport::moq::create_media_track(&mut broadcast, &audio_track.name, 2)
                .unwrap();
        let video =
            crate::transport::moq::create_media_track(&mut broadcast, &video_track.name, 2)
                .unwrap();

        let catalog_producer = MoqPeerNode::create_and_publish_catalog(
            &mut broadcast,
            Some(&audio_track),
            Some(&video_track),
            640,
            480,
            VideoCodec::Vp9,
            AudioCodec::Opus,
        )
        .unwrap();

        PublisherBroadcastFixture {
            _origin: origin,
            _broadcast: broadcast,
            _catalog: catalog_producer,
            audio,
            video,
            consumer,
        }
    }

    #[tokio::test]
    async fn process_publisher_frames_emits_video_packets_with_metadata() {
        let mut fx = make_single_track("video/data").await;
        write_group(&mut fx.producer, &[(1000, b"key0"), (2000, b"delta1")]);
        write_group(&mut fx.producer, &[(3000, b"key1")]);
        fx.producer.finish().unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let exit = MoqPeerNode::process_publisher_frames(
            fx.consumer,
            sender,
            "video/data",
            true,
            &mut shutdown_rx,
            &stats_tx,
            &dynamic_outputs,
            VideoCodec::Vp9,
        )
        .await;
        assert!(matches!(exit, TrackExit::Finished));

        let packets = mock.get_packets_for_pin("video/data").await;
        assert_eq!(packets.len(), 3);

        let metas: Vec<_> = packets
            .iter()
            .map(|p| match p {
                Packet::Binary { content_type, metadata, .. } => {
                    (content_type.clone(), metadata.clone().unwrap())
                },
                other => panic!("expected binary packet, got {other:?}"),
            })
            .collect();

        // Base timestamp (1000us) is subtracted from every frame.
        assert_eq!(metas[0].1.timestamp_us, Some(0));
        assert_eq!(metas[0].1.keyframe, Some(true));
        assert_eq!(metas[1].1.timestamp_us, Some(1000));
        assert_eq!(metas[1].1.keyframe, Some(false));
        // First frame of the second group is a fresh keyframe boundary.
        assert_eq!(metas[2].1.timestamp_us, Some(2000));
        assert_eq!(metas[2].1.keyframe, Some(true));
        for (ct, _) in &metas {
            assert_eq!(ct.as_deref(), Some(VP9_CONTENT_TYPE));
        }

        let stats = drain_stats(&mut stats_rx);
        assert_eq!(stats.received, 3);
        assert_eq!(stats.sent, 3);
    }

    #[tokio::test]
    async fn process_publisher_frames_audio_has_no_content_type() {
        let mut fx = make_single_track("audio/data").await;
        write_group(&mut fx.producer, &[(0, b"opus0")]);
        fx.producer.finish().unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let exit = MoqPeerNode::process_publisher_frames(
            fx.consumer,
            sender,
            "audio/data",
            false,
            &mut shutdown_rx,
            &stats_tx,
            &dynamic_outputs,
            VideoCodec::Vp9,
        )
        .await;
        assert!(matches!(exit, TrackExit::Finished));

        let packets = mock.get_packets_for_pin("audio/data").await;
        assert_eq!(packets.len(), 1);
        match &packets[0] {
            Packet::Binary { content_type, .. } => assert!(content_type.is_none()),
            other => panic!("expected binary packet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn process_publisher_frames_discards_undecodable_frame() {
        let mut fx = make_single_track("audio/data").await;
        let mut group = fx.producer.append_group().unwrap();
        // Empty payload has no varint timestamp — decode fails (DecodeError::Short).
        group
            .write_frame(hang::container::Timestamp::from_micros(0).unwrap(), bytes::Bytes::new())
            .unwrap();
        container_frame(0, b"ok").write_to(&mut group).unwrap();
        group.finish().unwrap();
        fx.producer.finish().unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let exit = MoqPeerNode::process_publisher_frames(
            fx.consumer,
            sender,
            "audio/data",
            false,
            &mut shutdown_rx,
            &stats_tx,
            &dynamic_outputs,
            VideoCodec::Vp9,
        )
        .await;
        assert!(matches!(exit, TrackExit::Finished));

        let packets = mock.get_packets_for_pin("audio/data").await;
        assert_eq!(packets.len(), 1, "the decodable frame should still be emitted");
        let stats = drain_stats(&mut stats_rx);
        assert_eq!(stats.discarded, 1);
    }

    #[tokio::test]
    async fn process_publisher_frames_routes_to_dynamic_output() {
        let mut fx = make_single_track("audio/data").await;
        write_group(&mut fx.producer, &[(0, b"opus0"), (20_000, b"opus1")]);
        fx.producer.finish().unwrap();

        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (dyn_tx, mut dyn_rx) = mpsc::channel::<Packet>(8);
        dynamic_outputs.write().unwrap().insert("audio/data".to_string(), dyn_tx);

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let exit = MoqPeerNode::process_publisher_frames(
            fx.consumer,
            sender,
            "audio/data",
            false,
            &mut shutdown_rx,
            &stats_tx,
            &dynamic_outputs,
            VideoCodec::Vp9,
        )
        .await;
        assert!(matches!(exit, TrackExit::Finished));

        let mut dyn_count = 0;
        while dyn_rx.try_recv().is_ok() {
            dyn_count += 1;
        }
        assert_eq!(dyn_count, 2, "frames should go to the dynamic channel");
        assert!(mock.try_recv().await.is_none(), "static sender should be bypassed");
    }

    #[tokio::test]
    async fn get_next_group_returns_group_then_finished() {
        let mut fx = make_single_track("video/data").await;
        write_group(&mut fx.producer, &[(0, b"a")]);
        fx.producer.finish().unwrap();
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let g1 =
            MoqPeerNode::get_next_group(&mut fx.consumer, &mut shutdown_rx, "video/data").await;
        assert!(matches!(g1, Ok(Some(_))));
        let g2 =
            MoqPeerNode::get_next_group(&mut fx.consumer, &mut shutdown_rx, "video/data").await;
        assert!(matches!(g2, Ok(None)), "stream ended → Ok(None)");
    }

    #[tokio::test]
    async fn get_next_group_returns_none_on_shutdown() {
        let mut fx = make_single_track("video/data").await;
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
        shutdown_tx.send(()).unwrap();
        let g = MoqPeerNode::get_next_group(&mut fx.consumer, &mut shutdown_rx, "video/data").await;
        assert!(matches!(g, Ok(None)));
    }

    #[tokio::test]
    async fn route_packet_prefers_dynamic_channel() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (dtx, mut drx) = mpsc::channel::<Packet>(4);
        dynamic_outputs.write().unwrap().insert("audio/data".to_string(), dtx);

        let mock = crate::test_utils::MockOutputSender::new();
        let mut sender = mock.to_output_sender("test_node".to_string());
        let pkt = Packet::Binary {
            data: bytes::Bytes::from_static(b"x"),
            content_type: None,
            metadata: None,
        };

        assert!(MoqPeerNode::route_packet(pkt, "audio/data", &mut sender, &dynamic_outputs).await);
        assert!(drx.try_recv().is_ok());
        assert!(mock.try_recv().await.is_none());
    }

    #[tokio::test]
    async fn route_packet_removes_closed_dynamic_entry() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (dtx, drx) = mpsc::channel::<Packet>(4);
        dynamic_outputs.write().unwrap().insert("audio/data".to_string(), dtx);
        drop(drx);

        let mock = crate::test_utils::MockOutputSender::new();
        let mut sender = mock.to_output_sender("test_node".to_string());
        let pkt = Packet::Binary {
            data: bytes::Bytes::from_static(b"x"),
            content_type: None,
            metadata: None,
        };

        assert!(MoqPeerNode::route_packet(pkt, "audio/data", &mut sender, &dynamic_outputs).await);
        assert!(
            !dynamic_outputs.read().unwrap().contains_key("audio/data"),
            "stale dynamic entry should be removed"
        );
    }

    #[tokio::test]
    async fn route_packet_falls_through_to_static_sender() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let mock = crate::test_utils::MockOutputSender::new();
        let mut sender = mock.to_output_sender("test_node".to_string());
        let pkt = Packet::Binary {
            data: bytes::Bytes::from_static(b"y"),
            content_type: None,
            metadata: None,
        };

        assert!(MoqPeerNode::route_packet(pkt, "video/data", &mut sender, &dynamic_outputs).await);
        let pkts = mock.get_packets_for_pin("video/data").await;
        assert_eq!(pkts.len(), 1);
    }

    #[tokio::test]
    async fn route_packet_returns_false_on_closed_static_channel() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let mock = crate::test_utils::MockOutputSender::new();
        let mut sender = mock.to_output_sender("test_node".to_string());
        drop(mock);
        let pkt = Packet::Binary {
            data: bytes::Bytes::from_static(b"z"),
            content_type: None,
            metadata: None,
        };

        assert!(!MoqPeerNode::route_packet(pkt, "video/data", &mut sender, &dynamic_outputs).await);
    }

    #[tokio::test]
    async fn wait_for_broadcast_announcement_returns_none_on_shutdown() {
        let origin = moq_net::Origin::random().produce();
        let consumer = origin.consume();
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
        shutdown_tx.send(()).unwrap();
        let res = MoqPeerNode::wait_for_broadcast_announcement(consumer, "input", &mut shutdown_rx)
            .await
            .unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn publisher_receive_loop_routes_catalog_tracks_to_pins() {
        let mut fx = make_publisher_broadcast();
        write_group(&mut fx.audio, &[(0, b"a0"), (20_000, b"a1")]);
        fx.audio.finish().unwrap();
        write_group(&mut fx.video, &[(0, b"v0")]);
        fx.video.finish().unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            MoqPeerNode::publisher_receive_loop(
                fx.consumer,
                "input".to_string(),
                &mut shutdown_rx,
                test_routing(sender, stats_tx, dynamic_outputs),
            ),
        )
        .await
        .expect("publisher_receive_loop should not hang");
        assert!(result.is_ok());

        let all = mock.collect_packets().await;
        let audio = all.iter().filter(|(_, pin, _)| pin == "audio/data").count();
        let video = all.iter().filter(|(_, pin, _)| pin == "video/data").count();
        assert_eq!(audio, 2);
        assert_eq!(video, 1);
        let video_pkt = all.iter().find(|(_, pin, _)| pin == "video/data").unwrap();
        match &video_pkt.2 {
            Packet::Binary { content_type, .. } => {
                assert_eq!(content_type.as_deref(), Some(VP9_CONTENT_TYPE));
            },
            other => panic!("expected binary packet, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publisher_receive_loop_with_slot_processes_and_emits_events() {
        let mut fx = make_publisher_broadcast();
        write_group(&mut fx.audio, &[(0, b"a0")]);
        fx.audio.finish().unwrap();
        fx.video.finish().unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let slot = Arc::new(Semaphore::new(1));
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let config = PublisherReceiveLoopWithSlotConfig {
            subscribe: fx.consumer,
            broadcast_name: "input".to_string(),
            publisher_slot: slot,
            publisher_events: events_tx,
            publisher_path: "peer-1".to_string(),
            routing: test_routing(sender, stats_tx, dynamic_outputs),
        };

        let result = tokio::time::timeout(
            Duration::from_secs(10),
            MoqPeerNode::publisher_receive_loop_with_slot(config, &mut shutdown_rx),
        )
        .await
        .expect("publisher_receive_loop_with_slot should not hang");
        assert!(result.is_ok());

        assert!(matches!(events_rx.try_recv().unwrap(), PublisherEvent::Connected { .. }));
        assert!(matches!(events_rx.try_recv().unwrap(), PublisherEvent::Disconnected { .. }));
        let audio = mock.get_packets_for_pin("audio/data").await;
        assert_eq!(audio.len(), 1);
    }

    #[tokio::test]
    async fn publisher_receive_loop_with_slot_skips_when_no_permit() {
        let fx = make_publisher_broadcast();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (events_tx, mut events_rx) = mpsc::unbounded_channel();
        let slot = Arc::new(Semaphore::new(0));
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let config = PublisherReceiveLoopWithSlotConfig {
            subscribe: fx.consumer,
            broadcast_name: "input".to_string(),
            publisher_slot: slot,
            publisher_events: events_tx,
            publisher_path: "peer-1".to_string(),
            routing: test_routing(sender, stats_tx, dynamic_outputs),
        };

        let result = tokio::time::timeout(
            Duration::from_secs(5),
            MoqPeerNode::publisher_receive_loop_with_slot(config, &mut shutdown_rx),
        )
        .await
        .expect("publisher_receive_loop_with_slot should not hang");
        assert!(result.is_ok());
        assert!(events_rx.try_recv().is_err(), "no Connected event when the slot is unavailable");
    }

    #[tokio::test]
    async fn resolve_media_types_applies_already_resolved_state() {
        let (_tx, mut rx) =
            watch::channel(MediaTypeState { has_audio: true, has_video: true, resolved: true });
        let mut media = sub_media(false, false);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let ok =
            MoqPeerNode::resolve_media_types(&mut media, &mut rx, &mut shutdown_rx).await.unwrap();
        assert!(ok);
        assert!(media.has_audio && media.has_video);
    }

    #[tokio::test]
    async fn resolve_media_types_returns_false_on_shutdown() {
        let (_tx, mut rx) =
            watch::channel(MediaTypeState { has_audio: false, has_video: false, resolved: false });
        let mut media = sub_media(false, false);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
        shutdown_tx.send(()).unwrap();

        let ok =
            MoqPeerNode::resolve_media_types(&mut media, &mut rx, &mut shutdown_rx).await.unwrap();
        assert!(!ok);
    }

    #[tokio::test(start_paused = true)]
    async fn resolve_media_types_grace_period_times_out_with_partial_media() {
        let (_tx, mut rx) =
            watch::channel(MediaTypeState { has_audio: true, has_video: false, resolved: true });
        let mut media = sub_media(false, false);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let ok =
            MoqPeerNode::resolve_media_types(&mut media, &mut rx, &mut shutdown_rx).await.unwrap();
        assert!(ok);
        assert!(media.has_audio && !media.has_video);
    }

    #[tokio::test]
    async fn setup_subscriber_broadcast_publishes_catalog_with_both_tracks() {
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, true);
        let (bcast, audio, video, catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();
        assert!(audio.is_some());
        assert!(video.is_some());

        let bc = publish.consume().announced_broadcast("output").await.unwrap();
        let cat_track = crate::transport::moq::subscribe_catalog(&bc).await.unwrap();
        let mut cc = crate::transport::moq::catalog_consumer::CatalogConsumer::new(cat_track);
        let cat = tokio::time::timeout(Duration::from_secs(2), cc.next())
            .await
            .unwrap()
            .unwrap()
            .expect("a catalog frame should be published");
        assert!(cat.audio.renditions.contains_key("audio/data"));
        assert!(cat.video.renditions.contains_key("video/data"));
        assert_eq!(cat.video.renditions.get("video/data").unwrap().coded_width, Some(640));
        drop((bcast, catalog));
    }

    #[tokio::test]
    async fn setup_subscriber_broadcast_audio_only_omits_video_track() {
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, false);
        let (_bcast, audio, video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();
        assert!(audio.is_some());
        assert!(video.is_none());
    }

    #[tokio::test]
    async fn create_and_publish_catalog_aac_advertises_stereo() {
        let publish = moq_net::Origin::random().produce();
        let mut bcast = publish
            .create_broadcast("output", moq_net::broadcast::Route::announced())
            .unwrap();
        let audio_track =
            crate::transport::moq::TrackRef { name: "audio/data".to_string(), priority: 80 };
        let catalog = MoqPeerNode::create_and_publish_catalog(
            &mut bcast,
            Some(&audio_track),
            None,
            640,
            480,
            VideoCodec::Vp9,
            AudioCodec::Aac,
        )
        .unwrap();

        let bc = publish.consume().announced_broadcast("output").await.unwrap();
        let cat_track = crate::transport::moq::subscribe_catalog(&bc).await.unwrap();
        let mut cc = crate::transport::moq::catalog_consumer::CatalogConsumer::new(cat_track);
        let cat = tokio::time::timeout(Duration::from_secs(2), cc.next())
            .await
            .unwrap()
            .unwrap()
            .expect("a catalog frame should be published");
        assert_eq!(cat.audio.renditions.get("audio/data").unwrap().channel_count, 2);
        drop((bcast, catalog));
    }

    #[tokio::test]
    async fn run_subscriber_send_loop_forwards_audio_and_video() {
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, true);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();

        let (tx, rx) = broadcast::channel::<BroadcastFrame>(16);
        tx.send(audio_frame(b"a0")).unwrap();
        tx.send(video_frame(b"v0", true)).unwrap();
        tx.send(audio_frame(b"a1")).unwrap();
        drop(tx);

        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let count = drive_send_loop(&mut audio, &mut video, rx, &mut shutdown_rx, &stats_tx).await;
        assert_eq!(count, 3);
        drop(publish);
    }

    #[tokio::test]
    async fn run_subscriber_send_loop_records_lagged_discards() {
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, false);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();

        let (tx, rx) = broadcast::channel::<BroadcastFrame>(2);
        for _ in 0..6 {
            let _ = tx.send(audio_frame(b"a"));
        }
        drop(tx);

        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let count = drive_send_loop(&mut audio, &mut video, rx, &mut shutdown_rx, &stats_tx).await;
        let stats = drain_stats(&mut stats_rx);
        // capacity 2 + 6 sends before any recv yields exactly Lagged(4), then the
        // 2 buffered frames are delivered.
        assert_eq!(stats.discarded, 4, "lagged frames should be counted as discarded");
        assert_eq!(count, 2);
        drop(publish);
    }

    #[tokio::test]
    async fn run_subscriber_send_loop_stops_on_shutdown() {
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, true);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();

        // Keep the sender alive so recv() stays pending and shutdown wins.
        let (_tx, rx) = broadcast::channel::<BroadcastFrame>(4);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);
        shutdown_tx.send(()).unwrap();

        let count = drive_send_loop(&mut audio, &mut video, rx, &mut shutdown_rx, &stats_tx).await;
        assert_eq!(count, 0);
        drop(publish);
    }

    #[tokio::test]
    async fn run_subscriber_send_loop_drops_leading_video_deltas_until_keyframe() {
        // Regression test for the Monitor preview "Waiting for first frame…"
        // bug: a peer that attaches mid-GOP (e.g. the preview peer) must not
        // open its first MoQ group with a delta frame, or a late-joining
        // subscriber's decoder wedges on "a key frame is required after
        // configure()".
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(false, true);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();

        let bc = publish.consume().announced_broadcast("output").await.unwrap();
        let mut sub = crate::transport::moq::subscribe_track(&bc, "video/data", 60).await.unwrap();

        // The peer attaches mid-stream: two delta frames arrive before the
        // first keyframe. A trailing delta then lands in the keyframe-led
        // group, and a second keyframe opens a fresh group.
        let (tx, rx) = broadcast::channel::<BroadcastFrame>(16);
        tx.send(video_frame(b"d0", false)).unwrap();
        tx.send(video_frame(b"d1", false)).unwrap();
        tx.send(video_frame(b"key", true)).unwrap();
        tx.send(video_frame(b"d2", false)).unwrap();
        tx.send(video_frame(b"key2", true)).unwrap();
        tx.send(video_frame(b"d3", false)).unwrap();
        drop(tx);

        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let count = drive_send_loop(&mut audio, &mut video, rx, &mut shutdown_rx, &stats_tx).await;
        assert_eq!(count, 6, "every received frame is accounted for");

        let stats = drain_stats(&mut stats_rx);
        assert_eq!(stats.discarded, 2, "the two leading delta frames are dropped");

        // Close the final group/track so the group reads below terminate
        // instead of waiting for more frames.
        video.as_mut().unwrap().finish().unwrap();

        // The first group must open on the keyframe and carry the trailing
        // delta; a regression that opened a second, delta-led group would wedge
        // a late-joining decoder.
        assert_eq!(
            read_group_payloads(&mut sub).await,
            vec![b"key".to_vec(), b"d2".to_vec()],
            "first group is keyframe-led and includes its trailing delta"
        );
        assert_eq!(
            read_group_payloads(&mut sub).await,
            vec![b"key2".to_vec(), b"d3".to_vec()],
            "the next keyframe opens a fresh group"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), sub.next_group())
                .await
                .expect("next_group should not hang")
                .expect("next_group should succeed")
                .is_none(),
            "no further groups are produced"
        );

        drop(publish);
    }

    #[tokio::test]
    async fn lagged_video_keeps_forwarding_without_regating() {
        // A lag on the shared A/V broadcast channel carries no per-kind or
        // keyframe information, so it must not re-gate video. The current group
        // already opened on a keyframe and stays open across the lag, so a
        // post-lag delta keeps flowing into it (and a subscriber joining after
        // the lag still sees a keyframe-led group). Re-gating on a bare lag
        // count would freeze video until the next keyframe — up to a full GOP —
        // even for a lag that only dropped audio.
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(false, true);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();

        let bc = publish.consume().announced_broadcast("output").await.unwrap();
        let mut sub = crate::transport::moq::subscribe_track(&bc, "video/data", 60).await.unwrap();

        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let mut ctx = make_test_ctx(&mut audio, &mut video, &stats_tx, 40);

        let r = MoqPeerNode::handle_broadcast_recv(Ok(video_frame(b"k0", true)), &mut ctx).unwrap();
        assert!(matches!(r, SendResult::Continue));

        let r = MoqPeerNode::handle_broadcast_recv(
            Err(broadcast::error::RecvError::Lagged(2)),
            &mut ctx,
        )
        .unwrap();
        assert!(matches!(r, SendResult::Continue));
        assert_eq!(drain_stats(&mut stats_rx).discarded, 2);

        let r = MoqPeerNode::handle_broadcast_recv(Ok(video_frame(b"d", false)), &mut ctx).unwrap();
        assert!(matches!(r, SendResult::Continue));
        assert_eq!(
            drain_stats(&mut stats_rx).discarded,
            0,
            "a post-lag delta in an already-open group is forwarded, not dropped"
        );

        ctx.video_track_producer.as_mut().unwrap().finish().unwrap();
        assert_eq!(
            read_group_payloads(&mut sub).await,
            vec![b"k0".to_vec(), b"d".to_vec()],
            "video keeps forwarding into the keyframe-led group across the lag"
        );

        drop(publish);
    }

    #[tokio::test]
    async fn dropped_leading_video_deltas_stay_aligned_with_interleaved_audio() {
        // Finding #1: leading video deltas dropped before the first keyframe
        // must still advance the video clock. Audio is never gated, so if video
        // froze during the drop window the first kept keyframe would land behind
        // the audio that played alongside the dropped deltas and lip-sync would
        // drift permanently. Interleave audio with the dropped deltas (matched
        // 20ms durations) and assert the kept keyframe lands at the same media
        // time as the concurrent audio — on the wire, not just in a counter.
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, true);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();

        let bc = publish.consume().announced_broadcast("output").await.unwrap();
        let mut sub = crate::transport::moq::subscribe_track(&bc, "video/data", 60).await.unwrap();

        let video_delta = |data: &'static [u8]| BroadcastFrame {
            data: bytes::Bytes::from_static(data),
            duration_us: Some(20_000),
            kind: MediaKind::Video,
            keyframe: false,
        };

        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(256);
        let mut ctx = make_test_ctx(&mut audio, &mut video, &stats_tx, 40);

        MoqPeerNode::handle_broadcast_recv(Ok(audio_frame(b"a0")), &mut ctx).unwrap();
        MoqPeerNode::handle_broadcast_recv(Ok(video_delta(b"d0")), &mut ctx).unwrap();
        MoqPeerNode::handle_broadcast_recv(Ok(audio_frame(b"a1")), &mut ctx).unwrap();
        MoqPeerNode::handle_broadcast_recv(Ok(video_delta(b"d1")), &mut ctx).unwrap();

        // Two 20ms audio frames played while the two video deltas were dropped;
        // both clocks must read 40ms so the kept keyframe is aligned.
        assert_eq!(ctx.audio_clock.timestamp_us(), 40_000);
        assert_eq!(
            ctx.video_clock.timestamp_us(),
            ctx.audio_clock.timestamp_us(),
            "dropped video deltas keep the video clock level with audio"
        );
        assert_eq!(drain_stats(&mut stats_rx).discarded, 2);

        MoqPeerNode::handle_broadcast_recv(Ok(video_frame(b"k0", true)), &mut ctx).unwrap();
        ctx.video_track_producer.as_mut().unwrap().finish().unwrap();

        let mut group = tokio::time::timeout(Duration::from_secs(2), sub.next_group())
            .await
            .expect("next_group should not hang")
            .expect("next_group should succeed")
            .expect("a keyframe-led group should exist");
        let first = tokio::time::timeout(Duration::from_secs(2), group.read_frame())
            .await
            .expect("read_frame should not hang")
            .expect("read_frame should succeed")
            .expect("the keyframe should be published");
        let decoded = hang::container::Frame::decode(first.payload).unwrap();
        assert_eq!(decoded.payload.as_ref(), b"k0");
        assert_eq!(
            decoded.timestamp.as_micros(),
            40_000,
            "the kept keyframe lands at the same media time as the concurrent audio"
        );

        drop(publish);
    }

    #[tokio::test]
    async fn run_subscriber_send_loop_skips_frame_without_matching_producer() {
        let publish = moq_net::Origin::random().produce();
        let media = sub_media(true, false);
        let (_bcast, mut audio, mut video, _catalog) =
            MoqPeerNode::setup_subscriber_broadcast(&publish, "output", &media).unwrap();
        assert!(video.is_none());

        let (tx, rx) = broadcast::channel::<BroadcastFrame>(8);
        tx.send(video_frame(b"v", true)).unwrap();
        tx.send(audio_frame(b"a")).unwrap();
        drop(tx);

        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (_shutdown_tx, mut shutdown_rx) = broadcast::channel::<()>(1);

        let count = drive_send_loop(&mut audio, &mut video, rx, &mut shutdown_rx, &stats_tx).await;
        assert_eq!(count, 2, "both frames are counted even though the video frame is skipped");
        drop(publish);
    }

    #[tokio::test]
    async fn handle_pin_management_output_pin_lifecycle() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (btx, _brx) = broadcast::channel::<BroadcastFrame>(16);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
        let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        let codecs = MediaCodecConfig { video: VideoCodec::Av1, audio: AudioCodec::Opus };

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("video/hd".to_string()),
                response_tx: resp_tx,
            },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        let pin = resp_rx.await.unwrap().unwrap();
        assert_eq!(pin.name, "video/hd");
        assert!(
            matches!(&pin.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Av1)
        );

        let (data_tx, _data_rx) = mpsc::channel::<Packet>(4);
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::AddedOutputPin { pin, channel: data_tx },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        assert!(dynamic_outputs.read().unwrap().contains_key("video/hd"));

        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RemoveOutputPin { pin_name: "video/hd".to_string() },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        assert!(!dynamic_outputs.read().unwrap().contains_key("video/hd"));
    }

    #[tokio::test]
    async fn handle_pin_management_input_pin_lifecycle() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (btx, _brx) = broadcast::channel::<BroadcastFrame>(16);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
        let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        let codecs = MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus };

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RequestAddInputPin {
                suggested_name: Some("audio/extra".to_string()),
                response_tx: resp_tx,
            },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        assert_eq!(resp_rx.await.unwrap().unwrap().name, "audio/extra");

        let (_in_tx, in_rx) = mpsc::channel::<Packet>(4);
        let pin = InputPin {
            name: "audio/extra".to_string(),
            accepts_types: vec![],
            cardinality: PinCardinality::One,
        };
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::AddedInputPin { pin, channel: in_rx, hint_tx: None },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        assert!(handles.contains_key("audio/extra"));

        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RemoveInputPin { pin_name: "audio/extra".to_string() },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        assert!(!handles.contains_key("audio/extra"));
    }

    #[tokio::test]
    async fn dynamic_input_forwarder_forwards_packets_to_broadcast() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (btx, mut brx) = broadcast::channel::<BroadcastFrame>(16);
        let (stats_tx, mut stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
        let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        let codecs = MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus };

        let (in_tx, in_rx) = mpsc::channel::<Packet>(4);
        let pin = InputPin {
            name: "audio/extra".to_string(),
            accepts_types: vec![],
            cardinality: PinCardinality::One,
        };
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::AddedInputPin { pin, channel: in_rx, hint_tx: None },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );

        in_tx
            .send(Packet::Binary {
                data: bytes::Bytes::from_static(b"opus"),
                content_type: None,
                metadata: None,
            })
            .await
            .unwrap();
        let frame = tokio::time::timeout(Duration::from_secs(2), brx.recv())
            .await
            .expect("forwarder should publish promptly")
            .unwrap();
        assert_eq!(frame.kind, MediaKind::Audio);
        assert_eq!(&frame.data[..], b"opus");
        let stats = drain_stats(&mut stats_rx);
        assert_eq!(stats.received, 1);
        assert_eq!(stats.sent, 1);

        shutdown_tx.send(()).unwrap();
    }

    /// Regression for #529: a dynamic output pin must advertise the codec the
    /// remote publisher's catalog declared for that track — not the local
    /// `video_codec` config — otherwise downstream nodes receive mislabeled
    /// frames when peers use different codecs.
    #[tokio::test]
    async fn request_add_output_pin_prefers_catalog_discovered_codec() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (btx, _brx) = broadcast::channel::<BroadcastFrame>(16);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
        let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        // Local config says VP9 but the remote catalog advertised H264.
        let codecs = MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus };
        record_discovered_codec(
            &discovered_codecs,
            "video/hd",
            DiscoveredCodec::Video(VideoCodec::H264),
        );

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("video/hd".to_string()),
                response_tx: resp_tx,
            },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        let pin = resp_rx.await.unwrap().unwrap();
        assert!(
            matches!(&pin.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::H264),
            "pin should advertise the catalog-discovered codec, got {:?}",
            pin.produces_type
        );

        // Pins without a catalog entry still fall back to the local config.
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("video/sd".to_string()),
                response_tx: resp_tx,
            },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        let pin = resp_rx.await.unwrap().unwrap();
        assert!(
            matches!(&pin.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Vp9)
        );
    }

    /// A dynamic audio pin must advertise the catalog-discovered audio codec,
    /// not the local audio config — the audio analogue of #529.
    #[tokio::test]
    async fn request_add_output_pin_prefers_catalog_discovered_audio_codec() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (btx, _brx) = broadcast::channel::<BroadcastFrame>(16);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
        let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        // Local config says Opus but the remote catalog advertised AAC.
        let codecs = MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus };
        record_discovered_codec(
            &discovered_codecs,
            "audio/data",
            DiscoveredCodec::Audio(AudioCodec::Aac),
        );

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("audio/data".to_string()),
                response_tx: resp_tx,
            },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );
        let pin = resp_rx.await.unwrap().unwrap();
        assert!(
            matches!(&pin.produces_type, PacketType::EncodedAudio(fmt) if fmt.codec == AudioCodec::Aac),
            "pin should advertise the catalog-discovered audio codec, got {:?}",
            pin.produces_type
        );
    }

    /// `RemoveOutputPin` must prune the discovered-codec entry so a recreated
    /// pin doesn't inherit a stale codec from a previous publisher.
    #[tokio::test]
    async fn remove_output_pin_prunes_discovered_codec() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let (btx, _brx) = broadcast::channel::<BroadcastFrame>(16);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let (shutdown_tx, _shutdown_rx) = broadcast::channel::<()>(1);
        let mut handles: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
        let codecs = MediaCodecConfig { video: VideoCodec::Vp9, audio: AudioCodec::Opus };
        record_discovered_codec(
            &discovered_codecs,
            "video/hd",
            DiscoveredCodec::Video(VideoCodec::H264),
        );

        MoqPeerNode::handle_pin_management(
            PinManagementMessage::RemoveOutputPin { pin_name: "video/hd".to_string() },
            &dynamic_outputs,
            &discovered_codecs,
            &btx,
            &stats_tx,
            &shutdown_tx,
            &mut handles,
            codecs,
        );

        assert_eq!(discovered_codec(&discovered_codecs, "video/hd"), None);
    }

    /// Regression for #529: `subscribe_catalog_tracks` must label each video
    /// track with its catalog rendition codec, not the local `video_codec`.
    #[tokio::test]
    async fn subscribe_catalog_tracks_records_per_rendition_codec() {
        let mut catalog = hang::catalog::Catalog::default();
        catalog.video.renditions.insert("video/hd".to_string(), {
            let mut cfg = hang::catalog::VideoConfig::new(
                crate::transport::moq::constants::catalog_video_codec(VideoCodec::H264),
            );
            cfg.framerate = Some(30.0);
            cfg.optimize_for_latency = Some(true);
            cfg
        });

        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("input", moq_net::broadcast::Route::announced())
            .unwrap();
        let _track =
            crate::transport::moq::create_media_track(&mut broadcast, "video/hd", 2).unwrap();
        let consumer = origin.consume().announced_broadcast("input").await.unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let mut track_handles = HashMap::new();

        let routing = TrackRouting {
            output_sender: sender,
            stats_delta_tx: stats_tx,
            dynamic_outputs,
            discovered_codecs: discovered_codecs.clone(),
            video_codec: VideoCodec::Vp9,
        };
        MoqPeerNode::subscribe_catalog_tracks(
            &catalog,
            &consumer,
            &shutdown_rx,
            &routing,
            None,
            &mut track_handles,
        );

        assert_eq!(
            discovered_codec(&discovered_codecs, "video/hd"),
            Some(DiscoveredCodec::Video(VideoCodec::H264)),
            "catalog rendition codec (H264) should win over local config (VP9)"
        );

        shutdown_tx.send(()).unwrap();
        for (_, handle) in track_handles {
            handle.abort();
        }
    }

    /// Audio analogue of the per-rendition codec test: the catalog's audio
    /// codec (AAC) must be recorded for the pin, not the local config (Opus).
    #[tokio::test]
    async fn subscribe_catalog_tracks_records_audio_rendition_codec() {
        let mut catalog = hang::catalog::Catalog::default();
        catalog.audio.renditions.insert(
            "audio/data".to_string(),
            hang::catalog::AudioConfig::new(catalog_audio_codec(AudioCodec::Aac), 48000, 2),
        );

        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("input", moq_net::broadcast::Route::announced())
            .unwrap();
        let _track =
            crate::transport::moq::create_media_track(&mut broadcast, "audio/data", 2).unwrap();
        let consumer = origin.consume().announced_broadcast("input").await.unwrap();

        let mock = crate::test_utils::MockOutputSender::new();
        let sender = mock.to_output_sender("test_node".to_string());
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<()>(1);
        let (stats_tx, _stats_rx) = mpsc::channel::<NodeStatsDelta>(16);
        let discovered_codecs: DiscoveredCodecs = Arc::default();
        let mut track_handles = HashMap::new();

        let routing = TrackRouting {
            output_sender: sender,
            stats_delta_tx: stats_tx,
            dynamic_outputs: Arc::default(),
            discovered_codecs: discovered_codecs.clone(),
            video_codec: VideoCodec::Vp9,
        };
        MoqPeerNode::subscribe_catalog_tracks(
            &catalog,
            &consumer,
            &shutdown_rx,
            &routing,
            None,
            &mut track_handles,
        );

        assert_eq!(
            discovered_codec(&discovered_codecs, "audio/data"),
            Some(DiscoveredCodec::Audio(AudioCodec::Aac)),
            "catalog audio rendition codec (AAC) should be recorded for the pin"
        );

        shutdown_tx.send(()).unwrap();
        for (_, handle) in track_handles {
            handle.abort();
        }
    }
}
