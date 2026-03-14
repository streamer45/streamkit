// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Pull Node - subscribes to broadcasts from a MoQ server

use super::constants::DEFAULT_AUDIO_FRAME_DURATION_US;
use async_trait::async_trait;
use bytes::Buf;
use moq_lite::AsPath;
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::time::Duration;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketMetadata, PacketType,
    VideoCodec,
};
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};

#[derive(Deserialize, Debug, JsonSchema, Clone, Default)]
#[serde(default)]
pub struct MoqPullConfig {
    pub url: String,
    /// Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.
    ///
    /// This is compatible with moq-relay and StreamKit's built-in MoQ auth.
    pub jwt: Option<String>,
    pub broadcast: String,
}

/// A node that connects to a MoQ server, subscribes to a broadcast,
/// and outputs the received media as encoded packets.
///
/// This node performs catalog discovery during initialization and supports
/// both audio (Opus) and video (VP9) tracks.
///
/// **Output pins**
/// - Always exposes a stable `out` pin (audio) for backward-compatible pipelines.
/// - Also exposes one output pin per discovered track (by track name).
/// - At runtime, the node subscribes to the first discovered audio track and emits
///   its packets to both `out` and the track-named pin.
/// - Video tracks are output on their track-named pin (e.g. `video/data`).
pub struct MoqPullNode {
    config: MoqPullConfig,
    /// Dynamically discovered output pins (one per track)
    output_pins: Vec<OutputPin>,
}

impl MoqPullNode {
    pub fn new(config: MoqPullConfig) -> Self {
        Self {
            config,
            // Start with a single stable output pin.
            output_pins: vec![OutputPin {
                name: "out".to_string(),
                produces_type: PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                }),
                cardinality: PinCardinality::Broadcast,
            }],
        }
    }

    fn stable_out_pin() -> OutputPin {
        OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::EncodedAudio(EncodedAudioFormat {
                codec: AudioCodec::Opus,
                codec_private: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }
    }

    fn output_pins_for_tracks(tracks: &[moq_lite::Track]) -> Vec<OutputPin> {
        let mut pins = Vec::with_capacity(1 + tracks.len());
        pins.push(Self::stable_out_pin());
        for track in tracks {
            if track.name == "out" {
                continue;
            }
            // Track type is inferred from the track name prefix. The hang
            // protocol uses `audio/data` and `video/data` as canonical names,
            // so this aligns with the catalog-parsed codec info in parse_catalog().
            let produces_type = if track.name.starts_with("video/") {
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
            pins.push(OutputPin {
                name: track.name.clone(),
                produces_type,
                cardinality: PinCardinality::Broadcast,
            });
        }
        pins
    }
}

#[async_trait]
impl ProcessorNode for MoqPullNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![] // This is an input node.
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        self.output_pins.clone()
    }

    async fn initialize(
        &mut self,
        ctx: &streamkit_core::InitContext,
    ) -> Result<streamkit_core::pins::PinUpdate, StreamKitError> {
        tracing::info!(
            node_id = %ctx.node_id,
            url = %super::redact_url_str_for_logs(&self.config.url),
            broadcast = %self.config.broadcast,
            "MoqPullNode: Discovering tracks from broadcast catalog"
        );

        // Connect to the MoQ server and fetch the catalog
        let tracks = match self.discover_tracks().await {
            Ok(tracks) => tracks,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to discover tracks; using default output pin");
                // Fall back to single default output pin
                return Ok(streamkit_core::pins::PinUpdate::NoChange);
            },
        };

        // Create one output pin per discovered track
        if tracks.is_empty() {
            tracing::debug!("No tracks discovered, keeping default output pin");
            return Ok(streamkit_core::pins::PinUpdate::NoChange);
        }

        let new_output_pins = Self::output_pins_for_tracks(&tracks);
        for pin in &new_output_pins {
            tracing::info!(
                node_id = %ctx.node_id,
                pin = %pin.name,
                "MoqPullNode: Output pin available"
            );
        }

        // Update the node's output pins (use clone_from for efficiency)
        self.output_pins.clone_from(&new_output_pins);

        tracing::info!(
            node_id = %ctx.node_id,
            pin_count = new_output_pins.len(),
            "MoqPullNode: Successfully discovered {} output pins",
            new_output_pins.len()
        );

        Ok(streamkit_core::pins::PinUpdate::Updated { inputs: vec![], outputs: new_output_pins })
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);
        tracing::info!(
            url = %super::redact_url_str_for_logs(&self.config.url),
            broadcast = %self.config.broadcast,
            "MoqPullNode starting"
        );
        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut total_packet_count = 0;
        // Main reconnection loop - simple 1 second retry for all failures
        loop {
            match self.run_connection(&mut context, &mut total_packet_count).await {
                Ok(StreamEndReason::Natural) => {
                    tracing::info!(
                        "MoqPullNode finished successfully after {} total packets",
                        total_packet_count
                    );
                    break;
                },
                Ok(StreamEndReason::Reconnect) => {
                    state_helpers::emit_recovering(
                        &context.state_tx,
                        &node_name,
                        "Connection lost, retrying in 1s",
                        None,
                    );

                    tracing::warn!("MoqPullNode connection lost, retrying in 1s");

                    // Check for shutdown during sleep
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                        msg = context.control_rx.recv() => {
                            if matches!(msg, Some(streamkit_core::control::NodeControlMessage::Shutdown)) {
                                tracing::info!("MoQ pull received shutdown during retry wait");
                                break;
                            }
                        }
                    }

                    state_helpers::emit_running(&context.state_tx, &node_name);
                },
                Err(e) => {
                    // Check if this is a configuration error (unrecoverable)
                    if let StreamKitError::Configuration(_) = &e {
                        tracing::error!("MoqPullNode configuration error: {}", e);
                        state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
                        return Err(e);
                    }

                    // Treat other errors as transient, retry after 1s
                    state_helpers::emit_recovering(
                        &context.state_tx,
                        &node_name,
                        format!("Connection error, retrying in 1s: {e}"),
                        None,
                    );

                    tracing::warn!("MoqPullNode connection error, retrying in 1s: {}", e);

                    // Check for shutdown during sleep
                    tokio::select! {
                        () = tokio::time::sleep(Duration::from_secs(1)) => {}
                        msg = context.control_rx.recv() => {
                            if matches!(msg, Some(streamkit_core::control::NodeControlMessage::Shutdown)) {
                                tracing::info!("MoQ pull received shutdown during retry wait");
                                break;
                            }
                        }
                    }

                    state_helpers::emit_running(&context.state_tx, &node_name);
                },
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
        Ok(())
    }
}

/// Indicates why a MoQ stream ended, used for reconnection logic.
#[derive(Debug)]
enum StreamEndReason {
    /// Stream ended gracefully as expected
    Natural,
    /// Stream ended unexpectedly and should trigger a reconnection attempt
    Reconnect,
}

impl MoqPullNode {
    fn strip_hang_timestamp_header(
        mut payload: bytes::Bytes,
    ) -> Result<(u64, bytes::Bytes), moq_lite::Error> {
        // hang protocol: frame payload is prefixed with a varint timestamp in microseconds.
        // We parse it and forward the remaining bytes (Opus frame data).
        let timestamp = hang::container::Timestamp::decode(&mut payload)?;
        #[allow(clippy::cast_possible_truncation)] // MoQ timestamps fit in u64
        let timestamp_us = timestamp.as_micros() as u64;
        Ok((timestamp_us, payload.copy_to_bytes(payload.remaining())))
    }

    /// Read the next raw MoQ frame, returning the payload and whether this
    /// frame is the first in a newly opened MoQ group (i.e. a keyframe
    /// boundary in the hang protocol).
    async fn read_next_raw_moq(
        track_consumer: &mut moq_lite::TrackConsumer,
        current_group: &mut Option<moq_lite::GroupConsumer>,
        is_first_in_group: &mut bool,
    ) -> Result<Option<bytes::Bytes>, moq_lite::Error> {
        loop {
            if current_group.is_none() {
                match track_consumer.next_group().await {
                    Ok(Some(group)) => {
                        *current_group = Some(group);
                        *is_first_in_group = true;
                    },
                    Ok(None) => return Ok(None),
                    Err(e) => return Err(e),
                }
            }

            let Some(group) = current_group.as_mut() else {
                continue;
            };

            match group.read_frame().await {
                Ok(Some(payload)) => return Ok(Some(payload)),
                Ok(None) => {
                    // Group ended; move to the next group.
                    *current_group = None;
                },
                Err(e) => {
                    // Drop this group and let caller decide reconnection/error handling.
                    *current_group = None;
                    return Err(e);
                },
            }
        }
    }

    /// Connects to the MoQ server once to discover available tracks from the catalog.
    /// This is used during initialization to create output pins dynamically.
    async fn discover_tracks(&self) -> Result<Vec<moq_lite::Track>, StreamKitError> {
        tracing::info!(
            url = %super::redact_url_str_for_logs(&self.config.url),
            broadcast = %self.config.broadcast,
            "Connecting to MoQ server to discover tracks"
        );

        let url = super::parse_moq_url(&self.config.url, self.config.jwt.as_deref())?;

        let client = super::shared_insecure_client()?;

        let origin = moq_lite::Origin::produce();
        let consumer = origin.consume();
        let _consumer_session =
            client.clone().with_consume(origin).connect(url).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Subscribe to the specified broadcast.
        //
        // During dynamic session initialization, the broadcast may not have been announced yet.
        // Treat this as "no tracks discovered" rather than a hard error: the runtime `run()` path
        // already waits for announcements and will connect once the broadcast appears.
        let Some(broadcast) = consumer.consume_broadcast(&self.config.broadcast) else {
            tracing::debug!(
                broadcast = %self.config.broadcast,
                "Broadcast not available during catalog discovery; using default output pin"
            );
            return Ok(Vec::new());
        };

        // Subscribe to the catalog track
        let raw_catalog_track =
            broadcast.subscribe_track(&hang::catalog::Catalog::default_track()).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to subscribe to catalog track: {e}"))
            })?;
        let mut catalog_consumer = hang::catalog::CatalogConsumer::new(raw_catalog_track);

        // Parse the catalog to discover tracks
        let tracks = self.parse_catalog(&mut catalog_consumer).await?;

        tracing::info!(
            track_count = tracks.len(),
            "Successfully discovered {} tracks from catalog",
            tracks.len()
        );

        Ok(tracks)
    }

    async fn parse_catalog(
        &self,
        catalog_consumer: &mut hang::catalog::CatalogConsumer,
    ) -> Result<Vec<moq_lite::Track>, StreamKitError> {
        const CATALOG_TIMEOUT: Duration = Duration::from_secs(30);
        const RETRY_DELAY: Duration = Duration::from_millis(100);

        let start = tokio::time::Instant::now();

        // Keep trying to get a catalog with tracks until timeout
        // Use 1s timeout per attempt, but retry within the same connection instead of failing
        loop {
            let catalog =
                match tokio::time::timeout(Duration::from_millis(1000), catalog_consumer.next())
                    .await
                {
                    Ok(Ok(Some(catalog))) => catalog,
                    Ok(Ok(None)) => {
                        return Err(StreamKitError::Runtime(
                            "Catalog track closed before receiving catalog update".to_string(),
                        ));
                    },
                    Ok(Err(e)) => {
                        return Err(StreamKitError::Runtime(format!(
                            "Failed to read catalog update: {e}"
                        )));
                    },
                    Err(_timeout) => {
                        // Timeout is not fatal - just means catalog isn't ready yet
                        // Check if we've exceeded the overall timeout
                        if start.elapsed() >= CATALOG_TIMEOUT {
                            return Err(StreamKitError::Runtime(format!(
                                "Timed out waiting for catalog after {} seconds",
                                CATALOG_TIMEOUT.as_secs()
                            )));
                        }
                        // Catalog not ready yet, wait a bit before trying again
                        tracing::trace!(
                            "Catalog not ready yet (timeout), retrying in {}ms...",
                            RETRY_DELAY.as_millis()
                        );
                        tokio::time::sleep(RETRY_DELAY).await;
                        continue;
                    },
                };

            let mut tracks = Vec::new();

            for (track_name, config) in catalog.audio.renditions {
                match config.codec {
                    hang::catalog::AudioCodec::Opus => {
                        tracing::info!(track = %track_name, "found opus audio track");
                        let track = moq_lite::Track { name: track_name, priority: 80 };
                        tracks.push(track);
                    },
                    codec => {
                        tracing::debug!(
                            "skipping non-opus audio track: {} (codec: {})",
                            track_name,
                            codec
                        );
                    },
                }
            }

            for (track_name, config) in catalog.video.renditions {
                match config.codec {
                    hang::catalog::VideoCodec::VP9(_) => {
                        tracing::info!(track = %track_name, "found VP9 video track");
                        let track = moq_lite::Track { name: track_name, priority: 60 };
                        tracks.push(track);
                    },
                    codec => {
                        tracing::debug!(
                            "skipping non-VP9 video track: {} (codec: {:?})",
                            track_name,
                            codec
                        );
                    },
                }
            }

            if !tracks.is_empty() {
                return Ok(tracks);
            }

            // Check if we've exceeded the overall timeout
            if start.elapsed() >= CATALOG_TIMEOUT {
                return Err(StreamKitError::Runtime(format!(
                    "No supported tracks found in catalog after {} seconds",
                    CATALOG_TIMEOUT.as_secs()
                )));
            }

            // Catalog is empty, wait a bit before checking for the next update
            tracing::trace!("Catalog has no supported tracks yet, waiting for next update...");
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }

    // MoQ connection state machine with multiplexed track handling and error recovery
    // High complexity is inherent to protocol handling (track management, object streaming, packet routing)
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    async fn run_connection(
        &self,
        context: &mut NodeContext,
        total_packet_count: &mut u32,
    ) -> Result<StreamEndReason, StreamKitError> {
        /// Which media kind produced a frame in the multiplexed select loop.
        enum ReadSource {
            Audio,
            Video,
        }

        /// Cap re-subscribe attempts per track to prevent tight loops if a
        /// re-subscribed track immediately ends again.
        const MAX_RESUBSCRIBE_ATTEMPTS: u32 = 3;

        let url = super::parse_moq_url(&self.config.url, self.config.jwt.as_deref())?;

        let client = super::shared_insecure_client()?;

        // Create origin for consuming broadcasts only (no publishing to avoid cycles)
        let origin = moq_lite::Origin::produce();
        let consumer = origin.consume();
        let _consumer_session =
            client.clone().with_consume(origin).connect(url).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Wait for broadcast to become available
        // Note: consume_broadcast() only works after announcement, so we primarily rely on announcements
        let broadcast = {
            let mut announcements = consumer.clone();

            // Try immediate consume first (works if broadcast already announced)
            if let Some(broadcast) = consumer.consume_broadcast(&self.config.broadcast) {
                tracing::info!("Broadcast '{}' is immediately available", self.config.broadcast);
                broadcast
            } else {
                // Wait for announcement
                tracing::debug!(
                    "Waiting for broadcast '{}' to be announced...",
                    self.config.broadcast
                );

                loop {
                    tokio::select! {
                            msg = context.control_rx.recv() => {
                                match msg {
                                    Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                                        tracing::info!("MoQ pull received shutdown signal while waiting for broadcast");
                                        return Ok(StreamEndReason::Natural);
                                    }
                                    Some(control_msg) => {
                                        tracing::debug!("MoQ pull received control message while waiting: {:?}", control_msg);
                                    }
                                    None => {
                                        // Control channel closed - engine is shutting down
                                        tracing::info!("MoQ pull control channel closed while waiting for broadcast");
                                        return Ok(StreamEndReason::Natural);
                                    }
                                }
                            }
                            Some((path, maybe_broadcast)) = announcements.announced() => {
                                if let Some(broadcast) = maybe_broadcast {
                                    // Compare paths without allocation - bind path to extend lifetime
                                    let announced_path = path.as_path();
                                    let path_str = announced_path.as_str();
                                    if path_str == self.config.broadcast {
                                        tracing::info!("Broadcast '{}' has been announced", self.config.broadcast);
                                        break broadcast;
                                    }
                                    // Different broadcast announced, continue waiting
                    tracing::trace!("Different broadcast announced: {}", path_str);
                                }
                            }
                            else => {
                                tracing::warn!("Announcement channel closed before broadcast '{}' was announced, will reconnect", self.config.broadcast);
                                return Ok(StreamEndReason::Reconnect);
                            }
                        }
                }
            }
        };

        tracing::info!("Subscribed to broadcast '{}'", self.config.broadcast);

        // Get the catalog to find available tracks
        let raw_catalog_track =
            broadcast.subscribe_track(&hang::catalog::Catalog::default_track()).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to subscribe to catalog track: {e}"))
            })?;
        let mut catalog_consumer = hang::catalog::CatalogConsumer::new(raw_catalog_track);

        tracing::debug!(
            "subscribed to catalog track: {}",
            hang::catalog::Catalog::default_track().name
        );

        // Wait for catalog data with timeout
        let discovered_tracks = self.parse_catalog(&mut catalog_consumer).await?;

        if discovered_tracks.is_empty() {
            return Err(StreamKitError::Runtime(
                "No supported tracks found in broadcast".to_string(),
            ));
        }

        // Find the first audio and video tracks. Track type is determined by
        // the hang protocol's canonical track name prefix (`audio/…` / `video/…`),
        // consistent with how parse_catalog() discovers them from codec info.
        let audio_track = discovered_tracks.iter().find(|t| !t.name.starts_with("video/"));
        let video_track = discovered_tracks.iter().find(|t| t.name.starts_with("video/"));

        if audio_track.is_none() && video_track.is_none() {
            return Err(StreamKitError::Runtime(
                "No audio or video tracks found in broadcast".to_string(),
            ));
        }

        // Subscribe to audio track
        let (mut audio_track_consumer, audio_track_pin_name, audio_track_pin_registered) =
            if let Some(track) = audio_track {
                tracing::info!("subscribing to audio track: {}", track.name);
                let pin_name = track.name.clone();
                let pin_registered = self.output_pins.iter().any(|p| p.name == pin_name);
                let consumer = broadcast.subscribe_track(track).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to subscribe to audio track: {e}"))
                })?;
                (Some(consumer), Some(pin_name), pin_registered)
            } else {
                (None, None, false)
            };

        // Subscribe to video track
        let (mut video_track_consumer, video_track_pin_name, video_track_pin_registered) =
            if let Some(track) = video_track {
                tracing::info!("subscribing to video track: {}", track.name);
                let pin_name = track.name.clone();
                let pin_registered = self.output_pins.iter().any(|p| p.name == pin_name);
                let consumer = broadcast.subscribe_track(track).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to subscribe to video track: {e}"))
                })?;
                (Some(consumer), Some(pin_name), pin_registered)
            } else {
                (None, None, false)
            };

        let mut audio_current_group: Option<moq_lite::GroupConsumer> = None;
        let mut video_current_group: Option<moq_lite::GroupConsumer> = None;

        let mut session_packet_count: u32 = 0;
        let mut last_audio_timestamp_us: Option<u64> = None;
        let mut last_video_timestamp_us: Option<u64> = None;
        // Separate clocks per media kind so that seeding from the first
        // audio timestamp doesn't skew video timing (and vice versa).
        let mut audio_clock = MediaClock::new(0);
        let mut video_clock = MediaClock::new(0);
        let mut audio_is_first_in_group = true;
        let mut video_is_first_in_group = true;
        let mut consecutive_cancels: u32 = 0;
        let mut last_payload_at = tokio::time::Instant::now();
        let mut audio_resubscribe_attempts: u32 = 0;
        let mut video_resubscribe_attempts: u32 = 0;

        // Stats tracking
        let node_name = context.output_sender.node_name().to_string();
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        tracing::info!(
            audio = audio_track.is_some(),
            video = video_track.is_some(),
            "starting to read frames from tracks"
        );

        loop {
            // Build per-kind read futures inline so tokio::select! can borrow
            // the track consumers without lifetime issues from closures.
            let audio_read = async {
                match audio_track_consumer.as_mut() {
                    Some(c) => {
                        Self::read_next_raw_moq(
                            c,
                            &mut audio_current_group,
                            &mut audio_is_first_in_group,
                        )
                        .await
                    },
                    None => std::future::pending().await,
                }
            };
            let video_read = async {
                match video_track_consumer.as_mut() {
                    Some(c) => {
                        Self::read_next_raw_moq(
                            c,
                            &mut video_current_group,
                            &mut video_is_first_in_group,
                        )
                        .await
                    },
                    None => std::future::pending().await,
                }
            };

            let (read_result, source): (Result<Option<bytes::Bytes>, moq_lite::Error>, ReadSource) =
                if let Some(token) = &context.cancellation_token {
                    tokio::select! {
                        () = token.cancelled() => {
                            tracing::info!("MoQ pull cancelled after {} packets", session_packet_count);
                            return Ok(StreamEndReason::Natural);
                        }
                        msg = context.control_rx.recv() => {
                            match msg {
                                Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                                    tracing::info!("MoQ pull received shutdown signal after {} packets", session_packet_count);
                                    return Ok(StreamEndReason::Natural);
                                }
                                Some(control_msg) => {
                                    tracing::debug!("MoQ pull received control message: {:?}", control_msg);
                                    continue;
                                }
                                None => {
                                    tracing::info!("MoQ pull control channel closed, shutting down after {} packets", session_packet_count);
                                    return Ok(StreamEndReason::Natural);
                                }
                            }
                        }
                        result = audio_read => (result, ReadSource::Audio),
                        result = video_read => (result, ReadSource::Video),
                    }
                } else {
                    tokio::select! {
                        msg = context.control_rx.recv() => {
                            match msg {
                                Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                                    tracing::info!("MoQ pull received shutdown signal after {} packets", session_packet_count);
                                    return Ok(StreamEndReason::Natural);
                                }
                                Some(control_msg) => {
                                    tracing::debug!("MoQ pull received control message: {:?}", control_msg);
                                    continue;
                                }
                                None => {
                                    tracing::info!("MoQ pull control channel closed, shutting down after {} packets", session_packet_count);
                                    return Ok(StreamEndReason::Natural);
                                }
                            }
                        }
                        result = audio_read => (result, ReadSource::Audio),
                        result = video_read => (result, ReadSource::Video),
                    }
                };

            match read_result {
                Ok(Some(payload)) => {
                    consecutive_cancels = 0;
                    last_payload_at = tokio::time::Instant::now();

                    // Reset re-subscribe attempts on successful payload so a
                    // track that worked for a while gets fresh attempts later.
                    match source {
                        ReadSource::Audio => audio_resubscribe_attempts = 0,
                        ReadSource::Video => video_resubscribe_attempts = 0,
                    }

                    session_packet_count += 1;
                    *total_packet_count += 1;
                    stats_tracker.received();

                    if session_packet_count.is_multiple_of(100) {
                        tracing::debug!(
                            "processed {} frames (total: {})",
                            session_packet_count,
                            *total_packet_count
                        );
                    }

                    let (timestamp_us, data) = match Self::strip_hang_timestamp_header(payload) {
                        Ok(result) => result,
                        Err(e) => {
                            tracing::warn!("Failed to decode frame timestamp: {e}");
                            stats_tracker.discarded();
                            continue;
                        },
                    };

                    let (last_ts, default_dur, clock, is_first_in_group) = match source {
                        ReadSource::Audio => (
                            &mut last_audio_timestamp_us,
                            DEFAULT_AUDIO_FRAME_DURATION_US,
                            &mut audio_clock,
                            &mut audio_is_first_in_group,
                        ),
                        ReadSource::Video => (
                            &mut last_video_timestamp_us,
                            crate::video::DEFAULT_VIDEO_FRAME_DURATION_US,
                            &mut video_clock,
                            &mut video_is_first_in_group,
                        ),
                    };

                    if last_ts.is_none() {
                        clock.seed_from_timestamp_us(timestamp_us);
                    }
                    let duration_us = last_ts
                        .and_then(|prev| timestamp_us.checked_sub(prev))
                        .filter(|d| *d > 0)
                        .or(Some(default_dur));

                    // Propagate keyframe info for video: the first frame
                    // after a new MoQ group is a keyframe in the hang protocol.
                    let keyframe = match source {
                        ReadSource::Video => {
                            let kf = *is_first_in_group;
                            *is_first_in_group = false;
                            Some(kf)
                        },
                        ReadSource::Audio => None,
                    };

                    let (content_type, metadata) = match source {
                        ReadSource::Video => (
                            Some(Cow::Borrowed("video/vp9")),
                            Some(PacketMetadata {
                                timestamp_us: Some(timestamp_us),
                                duration_us,
                                sequence: None,
                                keyframe,
                            }),
                        ),
                        ReadSource::Audio => (
                            None,
                            Some(PacketMetadata {
                                timestamp_us: Some(timestamp_us),
                                duration_us,
                                sequence: None,
                                keyframe: None,
                            }),
                        ),
                    };
                    *last_ts = Some(timestamp_us);

                    let packet = Packet::Binary { data, content_type, metadata: metadata.clone() };

                    // Route packet to the correct output pin(s)
                    match source {
                        ReadSource::Audio => {
                            let pin_name = audio_track_pin_name.as_deref().unwrap_or("out");
                            if audio_track_pin_registered
                                && pin_name != "out"
                                && context
                                    .output_sender
                                    .send(pin_name, packet.clone())
                                    .await
                                    .is_err()
                            {
                                tracing::debug!("Audio output channel closed, stopping node");
                                return Ok(StreamEndReason::Natural);
                            }
                            // Always send audio to the stable "out" pin
                            if context.output_sender.send("out", packet).await.is_err() {
                                tracing::debug!("Output channel closed, stopping node");
                                return Ok(StreamEndReason::Natural);
                            }
                        },
                        ReadSource::Video => {
                            if let Some(pin_name) = video_track_pin_name.as_deref() {
                                if video_track_pin_registered {
                                    if context.output_sender.send(pin_name, packet).await.is_err() {
                                        tracing::debug!(
                                            "Video output channel closed, stopping node"
                                        );
                                        return Ok(StreamEndReason::Natural);
                                    }
                                } else {
                                    tracing::trace!("Video pin not registered, discarding packet");
                                    stats_tracker.discarded();
                                    continue;
                                }
                            }
                        },
                    }
                    stats_tracker.sent();
                    stats_tracker.maybe_send();
                },
                Ok(None) => {
                    let kind = match source {
                        ReadSource::Audio => "audio",
                        ReadSource::Video => "video",
                    };
                    tracing::info!(
                        "{kind} track stream ended naturally after {session_packet_count} packets"
                    );

                    // Mark the ended track as inactive and reset its group state.
                    // Then attempt to re-subscribe in case the publisher
                    // re-announces the track (dynamic compositing scenario).
                    match source {
                        ReadSource::Audio => {
                            audio_track_consumer = None;
                            audio_current_group = None;
                            audio_is_first_in_group = true;
                            last_audio_timestamp_us = None;
                            audio_clock = MediaClock::new(0);

                            if audio_resubscribe_attempts < MAX_RESUBSCRIBE_ATTEMPTS {
                                if let Some(track) = audio_track {
                                    audio_resubscribe_attempts += 1;
                                    match broadcast.subscribe_track(track) {
                                        Ok(new_consumer) => {
                                            tracing::info!(
                                                attempt = audio_resubscribe_attempts,
                                                "re-subscribed to audio track after it ended"
                                            );
                                            audio_track_consumer = Some(new_consumer);
                                        },
                                        Err(e) => {
                                            tracing::debug!(
                                                error = %e,
                                                "could not re-subscribe to audio track"
                                            );
                                        },
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "audio track re-subscribe limit reached, giving up"
                                );
                            }
                        },
                        ReadSource::Video => {
                            video_track_consumer = None;
                            video_current_group = None;
                            video_is_first_in_group = true;
                            last_video_timestamp_us = None;
                            video_clock = MediaClock::new(0);

                            if video_resubscribe_attempts < MAX_RESUBSCRIBE_ATTEMPTS {
                                if let Some(track) = video_track {
                                    video_resubscribe_attempts += 1;
                                    match broadcast.subscribe_track(track) {
                                        Ok(new_consumer) => {
                                            tracing::info!(
                                                attempt = video_resubscribe_attempts,
                                                "re-subscribed to video track after it ended"
                                            );
                                            video_track_consumer = Some(new_consumer);
                                        },
                                        Err(e) => {
                                            tracing::debug!(
                                                error = %e,
                                                "could not re-subscribe to video track"
                                            );
                                        },
                                    }
                                }
                            } else {
                                tracing::warn!(
                                    "video track re-subscribe limit reached, giving up"
                                );
                            }
                        },
                    }

                    // Only terminate when ALL active tracks have ended.
                    if audio_track_consumer.is_none() && video_track_consumer.is_none() {
                        tracing::info!("all tracks have ended, finishing connection");
                        return Ok(StreamEndReason::Natural);
                    }
                },
                Err(moq_lite::Error::Cancel) => {
                    consecutive_cancels = consecutive_cancels.saturating_add(1);
                    tracing::debug!(
                        session_packet_count,
                        total_packet_count = *total_packet_count,
                        consecutive_cancels,
                        "Track read cancelled (skipping to next group)"
                    );

                    if last_payload_at.elapsed() > Duration::from_secs(5)
                        && consecutive_cancels >= 50
                    {
                        tracing::warn!(
                            session_packet_count,
                            total_packet_count = *total_packet_count,
                            consecutive_cancels,
                            elapsed_ms = last_payload_at.elapsed().as_millis(),
                            "Excessive track cancels without payloads; reconnecting"
                        );
                        return Ok(StreamEndReason::Reconnect);
                    }
                },
                Err(e) => {
                    tracing::error!(error = %e, session_packet_count, "Error reading from track");
                    if session_packet_count > 0 {
                        tracing::warn!(
                            "Track ended unexpectedly after {} packets - will retry",
                            session_packet_count
                        );
                        return Ok(StreamEndReason::Reconnect);
                    }
                    return Err(StreamKitError::Runtime(format!("Failed to read from track: {e}")));
                },
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn test_output_pins_for_tracks_includes_stable_out() {
        let tracks = vec![moq_lite::Track { name: "audio/data".to_string(), priority: 0 }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks);
        assert!(pins.iter().any(|p| p.name == "out"));
        assert!(pins.iter().any(|p| p.name == "audio/data"));
    }

    #[test]
    fn test_output_pins_for_tracks_dedupes_out_track_name() {
        let tracks = vec![moq_lite::Track { name: "out".to_string(), priority: 0 }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks);
        assert_eq!(pins.iter().filter(|p| p.name == "out").count(), 1);
    }

    #[test]
    fn test_strip_hang_timestamp_header() {
        let mut buf = BytesMut::new();
        hang::container::Timestamp::from_micros(123)
            .expect("valid timestamp")
            .encode(&mut buf)
            .expect("encode succeeds");
        buf.extend_from_slice(b"opus-frame-bytes");
        let payload = buf.freeze();

        let (ts, stripped) = match MoqPullNode::strip_hang_timestamp_header(payload) {
            Ok(stripped) => stripped,
            Err(e) => panic!("decode failed: {e}"),
        };
        assert_eq!(ts, 123);
        assert_eq!(&stripped[..], b"opus-frame-bytes");
    }
}
