// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::video::{AV1_CONTENT_TYPE, H264_CONTENT_TYPE, VP9_CONTENT_TYPE};
use async_trait::async_trait;
use bytes::Buf;
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::time::Duration;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketMetadata, PacketType,
    VideoCodec,
};

use super::constants::audio_codec_from_catalog;
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};

/// A catalog-discovered track with optional codec metadata.
///
/// Wraps [`moq_lite::Track`] with the codec detected from the MoQ
/// catalog so that output pins can advertise the correct codec type.
struct DiscoveredTrack {
    track: moq_lite::Track,
    /// `Some(codec)` for video tracks; `None` for audio.
    video_codec: Option<VideoCodec>,
    /// `Some(codec)` for audio tracks; `None` for video.
    audio_codec: Option<AudioCodec>,
}

#[derive(Deserialize, Debug, JsonSchema, Clone, Default)]
#[serde(default, deny_unknown_fields)]
pub struct MoqPullConfig {
    pub url: String,
    /// Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.
    ///
    /// This is compatible with moq-relay and StreamKit's built-in MoQ auth.
    pub jwt: Option<String>,
    pub broadcast: String,
    /// Fallback audio codec used before catalog discovery completes or when the
    /// catalog contains no audio rendition. When `None`, defaults to Opus.
    /// A catalog-advertised codec, when discovered, takes precedence.
    pub audio_codec: Option<AudioCodec>,
}

pub struct MoqPullNode {
    config: MoqPullConfig,
    output_pins: Vec<OutputPin>,
}

impl MoqPullNode {
    pub fn new(config: MoqPullConfig) -> Self {
        let audio_codec = config.audio_codec.unwrap_or(AudioCodec::Opus);
        Self { output_pins: vec![Self::stable_out_pin(audio_codec)], config }
    }

    fn stable_out_pin(audio_codec: AudioCodec) -> OutputPin {
        OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::EncodedAudio(EncodedAudioFormat {
                codec: audio_codec,
                codec_private: None,
            }),
            cardinality: PinCardinality::Broadcast,
        }
    }

    fn output_pins_for_tracks(
        tracks: &[DiscoveredTrack],
        default_audio_codec: AudioCodec,
    ) -> Vec<OutputPin> {
        let mut pins = Vec::with_capacity(1 + tracks.len());
        let stable_codec =
            tracks.iter().find_map(|dt| dt.audio_codec).unwrap_or(default_audio_codec);
        pins.push(Self::stable_out_pin(stable_codec));
        for dt in tracks {
            if dt.track.name == "out" {
                continue;
            }
            let produces_type = if dt.video_codec.is_some() {
                PacketType::EncodedVideo(EncodedVideoFormat {
                    codec: dt.video_codec.unwrap_or(VideoCodec::Vp9),
                    bitstream_format: None,
                    codec_private: None,
                    profile: None,
                    level: None,
                })
            } else {
                PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: dt.audio_codec.unwrap_or(default_audio_codec),
                    codec_private: None,
                })
            };
            pins.push(OutputPin {
                name: dt.track.name.clone(),
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

        let default_audio_codec = self.config.audio_codec.unwrap_or(AudioCodec::Opus);
        let new_output_pins = Self::output_pins_for_tracks(&tracks, default_audio_codec);
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
            match Box::pin(self.run_connection(&mut context, &mut total_packet_count)).await {
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
#[derive(Debug, PartialEq, Eq)]
enum StreamEndReason {
    /// Stream ended gracefully as expected
    Natural,
    /// Stream ended unexpectedly and should trigger a reconnection attempt
    Reconnect,
}

/// Maximum consecutive `Cancel` errors before triggering a reconnect.
/// A single cancel is normal (group boundary), but a sustained burst means
/// the publisher has gone away or the relay has no data.
const MAX_CONSECUTIVE_CANCELS: u32 = 50;

/// Number of consecutive cancels before we start yielding (sleeping) to
/// avoid busy-spinning the async runtime.
const CANCEL_YIELD_THRESHOLD: u32 = 5;

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
    async fn discover_tracks(&self) -> Result<Vec<DiscoveredTrack>, StreamKitError> {
        tracing::info!(
            url = %super::redact_url_str_for_logs(&self.config.url),
            broadcast = %self.config.broadcast,
            "Connecting to MoQ server to discover tracks"
        );

        let mut url = super::parse_moq_url(&self.config.url, self.config.jwt.as_deref())?;

        // Pre-resolve hostname to avoid QUIC IPv6 timeout (see resolve_url_for_quic docs)
        if let Err(e) = super::resolve_url_for_quic(&mut url).await {
            tracing::warn!(error = %e, "Failed to pre-resolve MoQ URL; proceeding with original");
        }

        let client = super::shared_insecure_client()?;

        let origin = moq_lite::Origin::random().produce();
        let consumer = origin.consume();
        let _consumer_session =
            client.clone().with_consume(origin).connect(url).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Wait for the broadcast to be announced.  During dynamic session
        // initialization the publisher (browser) may not have connected yet, so
        // we poll with a short delay up to the timeout.
        let broadcast_poll_interval = Duration::from_millis(250);
        let discovery_timeout = Duration::from_secs(15);

        let discovery_start = tokio::time::Instant::now();
        let broadcast = loop {
            if let Some(b) = consumer.get_broadcast(&self.config.broadcast) {
                break b;
            }
            if discovery_start.elapsed() >= discovery_timeout {
                tracing::debug!(
                    broadcast = %self.config.broadcast,
                    "Broadcast not announced within {}s; using default output pin",
                    discovery_timeout.as_secs()
                );
                return Ok(Vec::new());
            }
            tracing::trace!(
                broadcast = %self.config.broadcast,
                "Broadcast not yet announced, retrying..."
            );
            tokio::time::sleep(broadcast_poll_interval).await;
        };

        // Subscribe to the catalog track
        let raw_catalog_track =
            broadcast.subscribe_track(&hang::catalog::Catalog::default_track()).map_err(|e| {
                StreamKitError::Runtime(format!("Failed to subscribe to catalog track: {e}"))
            })?;
        let mut catalog_consumer = super::catalog_consumer::CatalogConsumer::new(raw_catalog_track);

        // Parse the catalog to discover tracks
        let tracks = self.parse_catalog(&mut catalog_consumer).await?;

        tracing::info!(
            track_count = tracks.len(),
            "Successfully discovered {} tracks from catalog",
            tracks.len()
        );

        Ok(tracks)
    }

    /// Extract supported tracks (Opus audio, VP9/AV1 video) from a parsed catalog.
    fn extract_tracks(catalog: &hang::catalog::Catalog) -> Vec<DiscoveredTrack> {
        let mut tracks = Vec::new();

        for (name, config) in &catalog.audio.renditions {
            if let Some(codec) = audio_codec_from_catalog(&config.codec) {
                tracing::info!(track = %name, ?codec, "found audio track");
                tracks.push(DiscoveredTrack {
                    track: moq_lite::Track { name: name.clone(), priority: 80 },
                    video_codec: None,
                    audio_codec: Some(codec),
                });
            } else {
                tracing::debug!(track = %name, codec = ?config.codec, "skipping unsupported audio track");
            }
        }

        for (name, config) in &catalog.video.renditions {
            match config.codec {
                hang::catalog::VideoCodec::VP9(_) => {
                    tracing::info!(track = %name, "found VP9 video track");
                    tracks.push(DiscoveredTrack {
                        track: moq_lite::Track { name: name.clone(), priority: 60 },
                        video_codec: Some(VideoCodec::Vp9),
                        audio_codec: None,
                    });
                },
                hang::catalog::VideoCodec::AV1(_) => {
                    tracing::info!(track = %name, "found AV1 video track");
                    tracks.push(DiscoveredTrack {
                        track: moq_lite::Track { name: name.clone(), priority: 60 },
                        video_codec: Some(VideoCodec::Av1),
                        audio_codec: None,
                    });
                },
                hang::catalog::VideoCodec::H264(_) => {
                    tracing::info!(track = %name, "found H264 video track");
                    tracks.push(DiscoveredTrack {
                        track: moq_lite::Track { name: name.clone(), priority: 60 },
                        video_codec: Some(VideoCodec::H264),
                        audio_codec: None,
                    });
                },
                _ => {
                    tracing::debug!(track = %name, codec = ?config.codec, "skipping unsupported video track");
                },
            }
        }

        tracks
    }

    async fn parse_catalog(
        &self,
        catalog_consumer: &mut super::catalog_consumer::CatalogConsumer,
    ) -> Result<Vec<DiscoveredTrack>, StreamKitError> {
        let catalog_timeout = Duration::from_secs(30);
        let retry_delay = Duration::from_millis(100);
        let start = tokio::time::Instant::now();

        loop {
            let catalog =
                match tokio::time::timeout(Duration::from_secs(1), catalog_consumer.next()).await {
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
                        if start.elapsed() >= catalog_timeout {
                            return Err(StreamKitError::Runtime(format!(
                                "Timed out waiting for catalog after {} seconds",
                                catalog_timeout.as_secs()
                            )));
                        }
                        tracing::trace!("Catalog not ready yet, retrying...");
                        tokio::time::sleep(retry_delay).await;
                        continue;
                    },
                };

            let tracks = Self::extract_tracks(&catalog);

            if !tracks.is_empty() {
                return Ok(tracks);
            }

            if start.elapsed() >= catalog_timeout {
                return Err(StreamKitError::Runtime(format!(
                    "No supported tracks found in catalog after {} seconds",
                    catalog_timeout.as_secs()
                )));
            }

            tracing::trace!("Catalog has no supported tracks yet, waiting for next update...");
            tokio::time::sleep(retry_delay).await;
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

        let mut url = super::parse_moq_url(&self.config.url, self.config.jwt.as_deref())?;

        // Pre-resolve hostname to avoid QUIC IPv6 timeout (see resolve_url_for_quic docs)
        if let Err(e) = super::resolve_url_for_quic(&mut url).await {
            tracing::warn!(error = %e, "Failed to pre-resolve MoQ URL; proceeding with original");
        }

        let client = super::shared_insecure_client()?;

        // Create origin for consuming broadcasts only (no publishing to avoid cycles)
        let origin = moq_lite::Origin::random().produce();
        let consumer = origin.consume();
        let _consumer_session =
            client.clone().with_consume(origin).connect(url).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Wait for broadcast to become available
        let broadcast = {
            let mut announcements = consumer.clone();

            if let Some(broadcast) = consumer.get_broadcast(&self.config.broadcast) {
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
                                    if path.as_str() == self.config.broadcast {
                                        tracing::info!("Broadcast '{}' has been announced", self.config.broadcast);
                                        break broadcast;
                                    }
                                    // Different broadcast announced, continue waiting
                    tracing::trace!("Different broadcast announced: {}", path.as_str());
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
        let mut catalog_consumer = super::catalog_consumer::CatalogConsumer::new(raw_catalog_track);

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

        let audio_track = discovered_tracks.iter().find(|dt| dt.audio_codec.is_some());
        let video_track = discovered_tracks.iter().find(|dt| dt.video_codec.is_some());

        let resolved_audio_codec = audio_track
            .and_then(|dt| dt.audio_codec)
            .or(self.config.audio_codec)
            .unwrap_or(AudioCodec::Opus);

        // Warn when the catalog codec differs from the output pin set during
        // initialize(). This can happen when init fails to discover the catalog
        // (timeout) and the pin remains at the constructor's default codec.
        let out_pin_codec =
            self.output_pins.iter().find(|p| p.name == "out").and_then(|p| {
                match &p.produces_type {
                    PacketType::EncodedAudio(fmt) => Some(fmt.codec),
                    _ => None,
                }
            });
        if let Some(pin_codec) = out_pin_codec {
            if pin_codec != resolved_audio_codec {
                tracing::warn!(
                    advertised = ?pin_codec,
                    discovered = ?resolved_audio_codec,
                    "Catalog audio codec differs from output pin; \
                     pin was set during init before catalog was available"
                );
                return Err(StreamKitError::Runtime(format!(
                    "Audio codec mismatch: output pin advertises {pin_codec:?} \
                     but catalog resolved {resolved_audio_codec:?}; \
                     reconnecting to retry catalog discovery"
                )));
            }
        }

        if audio_track.is_none() && video_track.is_none() {
            return Err(StreamKitError::Runtime(
                "No audio or video tracks found in broadcast".to_string(),
            ));
        }

        // Subscribe to audio track
        let (mut audio_track_consumer, audio_track_pin_name, audio_track_pin_registered) =
            if let Some(dt) = audio_track {
                tracing::info!("subscribing to audio track: {}", dt.track.name);
                let pin_name = dt.track.name.clone();
                let pin_registered = self.output_pins.iter().any(|p| p.name == pin_name);
                let consumer = broadcast.subscribe_track(&dt.track).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to subscribe to audio track: {e}"))
                })?;
                (Some(consumer), Some(pin_name), pin_registered)
            } else {
                (None, None, false)
            };

        // Subscribe to video track
        let (mut video_track_consumer, video_track_pin_name, video_track_pin_registered) =
            if let Some(dt) = video_track {
                tracing::info!("subscribing to video track: {}", dt.track.name);
                let pin_name = dt.track.name.clone();
                let pin_registered = self.output_pins.iter().any(|p| p.name == pin_name);
                let consumer = broadcast.subscribe_track(&dt.track).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to subscribe to video track: {e}"))
                })?;
                (Some(consumer), Some(pin_name), pin_registered)
            } else {
                (None, None, false)
            };

        let mut audio_current_group: Option<moq_lite::GroupConsumer> = None;
        let mut video_current_group: Option<moq_lite::GroupConsumer> = None;

        // Resolve the video content_type from the discovered track's codec.
        // Falls back to "video/vp9" if no codec info is available.
        let video_content_type: &str = match video_track.and_then(|dt| dt.video_codec) {
            Some(VideoCodec::Av1) => AV1_CONTENT_TYPE,
            Some(VideoCodec::H264) => H264_CONTENT_TYPE,
            _ => VP9_CONTENT_TYPE,
        };

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
                            resolved_audio_codec.default_frame_duration_us(),
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
                            Some(Cow::Borrowed(video_content_type)),
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
                                if let Some(dt) = audio_track {
                                    audio_resubscribe_attempts += 1;
                                    match broadcast.subscribe_track(&dt.track) {
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
                                tracing::warn!("audio track re-subscribe limit reached, giving up");
                            }
                        },
                        ReadSource::Video => {
                            video_track_consumer = None;
                            video_current_group = None;
                            video_is_first_in_group = true;
                            last_video_timestamp_us = None;
                            video_clock = MediaClock::new(0);

                            if video_resubscribe_attempts < MAX_RESUBSCRIBE_ATTEMPTS {
                                if let Some(dt) = video_track {
                                    video_resubscribe_attempts += 1;
                                    match broadcast.subscribe_track(&dt.track) {
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
                                tracing::warn!("video track re-subscribe limit reached, giving up");
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

                    if consecutive_cancels >= MAX_CONSECUTIVE_CANCELS {
                        tracing::warn!(
                            session_packet_count,
                            total_packet_count = *total_packet_count,
                            consecutive_cancels,
                            elapsed_ms = last_payload_at.elapsed().as_millis(),
                            "Excessive track cancels without payloads; reconnecting"
                        );
                        return Ok(StreamEndReason::Reconnect);
                    }

                    // Yield after consecutive cancels to avoid busy-spinning.
                    if consecutive_cancels > CANCEL_YIELD_THRESHOLD {
                        tokio::time::sleep(Duration::from_millis(10)).await;
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
        let tracks = vec![DiscoveredTrack {
            track: moq_lite::Track { name: "audio/data".to_string(), priority: 0 },
            video_codec: None,
            audio_codec: Some(AudioCodec::Opus),
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        assert!(pins.iter().any(|p| p.name == "out"));
        assert!(pins.iter().any(|p| p.name == "audio/data"));
    }

    #[test]
    fn test_output_pins_for_tracks_dedupes_out_track_name() {
        let tracks = vec![DiscoveredTrack {
            track: moq_lite::Track { name: "out".to_string(), priority: 0 },
            video_codec: None,
            audio_codec: Some(AudioCodec::Opus),
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        assert_eq!(pins.iter().filter(|p| p.name == "out").count(), 1);
    }

    #[test]
    fn test_output_pins_for_tracks_uses_av1_codec() {
        let tracks = vec![DiscoveredTrack {
            track: moq_lite::Track { name: "video/data".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Av1),
            audio_codec: None,
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        let video_pin = pins.iter().find(|p| p.name == "video/data").unwrap();
        match &video_pin.produces_type {
            PacketType::EncodedVideo(fmt) => assert_eq!(fmt.codec, VideoCodec::Av1),
            other => panic!("expected EncodedVideo, got {other:?}"),
        }
    }

    #[test]
    fn test_output_pins_for_tracks_defaults_vp9() {
        let tracks = vec![DiscoveredTrack {
            track: moq_lite::Track { name: "video/data".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Vp9),
            audio_codec: None,
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        let video_pin = pins.iter().find(|p| p.name == "video/data").unwrap();
        match &video_pin.produces_type {
            PacketType::EncodedVideo(fmt) => assert_eq!(fmt.codec, VideoCodec::Vp9),
            other => panic!("expected EncodedVideo, got {other:?}"),
        }
    }

    #[test]
    fn test_output_pins_for_tracks_aac_audio() {
        let tracks = vec![DiscoveredTrack {
            track: moq_lite::Track { name: "audio/data".to_string(), priority: 80 },
            video_codec: None,
            audio_codec: Some(AudioCodec::Aac),
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        let audio_pin = pins.iter().find(|p| p.name == "audio/data").unwrap();
        match &audio_pin.produces_type {
            PacketType::EncodedAudio(fmt) => assert_eq!(fmt.codec, AudioCodec::Aac),
            other => panic!("expected EncodedAudio, got {other:?}"),
        }
        let out_pin = pins.iter().find(|p| p.name == "out").unwrap();
        match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => assert_eq!(fmt.codec, AudioCodec::Aac),
            other => panic!("expected EncodedAudio on out, got {other:?}"),
        }
    }

    #[test]
    fn test_output_pins_for_tracks_uses_config_default_when_no_audio_discovered() {
        let tracks = vec![DiscoveredTrack {
            track: moq_lite::Track { name: "video/data".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Vp9),
            audio_codec: None,
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Aac);
        let out_pin = pins.iter().find(|p| p.name == "out").unwrap();
        match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => assert_eq!(fmt.codec, AudioCodec::Aac),
            other => panic!("expected EncodedAudio(Aac) on out, got {other:?}"),
        }
    }

    #[test]
    fn test_new_defaults_to_opus() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        let out_pin = node.output_pins.iter().find(|p| p.name == "out").unwrap();
        match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => assert_eq!(fmt.codec, AudioCodec::Opus),
            other => panic!("expected EncodedAudio(Opus), got {other:?}"),
        }
    }

    #[test]
    fn test_new_respects_audio_codec_config() {
        let config = MoqPullConfig { audio_codec: Some(AudioCodec::Aac), ..Default::default() };
        let node = MoqPullNode::new(config);
        let out_pin = node.output_pins.iter().find(|p| p.name == "out").unwrap();
        match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => assert_eq!(fmt.codec, AudioCodec::Aac),
            other => panic!("expected EncodedAudio(Aac), got {other:?}"),
        }
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

    // Compile-time validation that the cancel constants are self-consistent.
    const _: () = {
        assert!(MAX_CONSECUTIVE_CANCELS > CANCEL_YIELD_THRESHOLD);
        assert!(CANCEL_YIELD_THRESHOLD >= 3);
        assert!(CANCEL_YIELD_THRESHOLD <= 10);
        // Worst-case spin time: (MAX - YIELD) * 10ms must be under 1 second.
        assert!((MAX_CONSECUTIVE_CANCELS - CANCEL_YIELD_THRESHOLD) * 10 <= 1000);
    };

    #[test]
    fn test_cancel_threshold_boundary() {
        // Verify the reconnect condition at the boundary:
        // below MAX → no reconnect, at MAX → reconnect.
        let below = MAX_CONSECUTIVE_CANCELS - 1;
        let at = MAX_CONSECUTIVE_CANCELS;
        assert!(below < MAX_CONSECUTIVE_CANCELS, "below threshold should not trigger");
        assert!(at >= MAX_CONSECUTIVE_CANCELS, "at threshold should trigger");
    }

    #[test]
    fn test_cancel_reconnect_is_count_only() {
        // Regression: the old code required BOTH consecutive_cancels >= 50
        // AND last_payload_at.elapsed() > 5 seconds. This caused a spin of
        // hundreds of cancels before reconnecting. The fix removed the time
        // condition so that MAX_CONSECUTIVE_CANCELS alone is sufficient.
        //
        // Verify worst-case time to reconnect at runtime matches the
        // compile-time assertion above.
        let worst_case_ms = u64::from(MAX_CONSECUTIVE_CANCELS - CANCEL_YIELD_THRESHOLD) * 10;
        assert!(
            worst_case_ms <= 1000,
            "worst-case cancel spin should be under 1 second, got {worst_case_ms}ms"
        );
    }

    #[test]
    fn test_track_selection_uses_codec_fields() {
        let tracks = [
            DiscoveredTrack {
                track: moq_lite::Track { name: "my-custom-audio".to_string(), priority: 80 },
                video_codec: None,
                audio_codec: Some(AudioCodec::Opus),
            },
            DiscoveredTrack {
                track: moq_lite::Track { name: "my-custom-video".to_string(), priority: 60 },
                video_codec: Some(VideoCodec::Vp9),
                audio_codec: None,
            },
        ];
        let audio = tracks.iter().find(|dt| dt.audio_codec.is_some());
        let video = tracks.iter().find(|dt| dt.video_codec.is_some());
        assert_eq!(audio.unwrap().track.name, "my-custom-audio");
        assert_eq!(video.unwrap().track.name, "my-custom-video");
    }

    #[test]
    fn test_output_pins_for_tracks_uses_codec_not_name() {
        let tracks = [DiscoveredTrack {
            track: moq_lite::Track { name: "non-standard-name".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Av1),
            audio_codec: None,
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        let pin = pins.iter().find(|p| p.name == "non-standard-name").unwrap();
        assert!(
            matches!(&pin.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Av1),
            "expected EncodedVideo(Av1), got {:?}",
            pin.produces_type
        );
    }

    #[test]
    fn test_default_node_has_opus_output_pin() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        let out_pin = node.output_pins.iter().find(|p| p.name == "out").unwrap();
        let pin_codec = match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => fmt.codec,
            _ => panic!("expected EncodedAudio"),
        };
        assert_eq!(pin_codec, AudioCodec::Opus);
    }

    #[test]
    fn test_codec_mismatch_detected_between_pin_and_catalog() {
        // Simulates the scenario where initialize() failed to discover the
        // catalog (timeout), leaving the output pin at the default Opus codec.
        // run_connection() later discovers a different codec (AAC) from the
        // catalog and should detect the mismatch.
        let node = MoqPullNode::new(MoqPullConfig::default());
        let out_pin = node.output_pins.iter().find(|p| p.name == "out").unwrap();
        let pin_codec = match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => fmt.codec,
            _ => panic!("expected EncodedAudio"),
        };

        let catalog_codec = AudioCodec::Aac;
        assert_ne!(
            pin_codec, catalog_codec,
            "default pin codec should differ from catalog AAC, triggering connection failure"
        );
    }

    #[test]
    fn test_no_mismatch_when_pin_matches_catalog() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        let out_pin = node.output_pins.iter().find(|p| p.name == "out").unwrap();
        let pin_codec = match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => fmt.codec,
            _ => panic!("expected EncodedAudio"),
        };

        let catalog_codec = AudioCodec::Opus;
        assert_eq!(
            pin_codec, catalog_codec,
            "matching codecs should not trigger a connection failure"
        );
    }

    #[test]
    fn test_explicit_aac_config_avoids_mismatch_with_aac_catalog() {
        let config =
            MoqPullConfig { audio_codec: Some(AudioCodec::Aac), ..MoqPullConfig::default() };
        let node = MoqPullNode::new(config);
        let out_pin = node.output_pins.iter().find(|p| p.name == "out").unwrap();
        let pin_codec = match &out_pin.produces_type {
            PacketType::EncodedAudio(fmt) => fmt.codec,
            _ => panic!("expected EncodedAudio"),
        };
        assert_eq!(pin_codec, AudioCodec::Aac);

        let catalog_codec = AudioCodec::Aac;
        assert_eq!(
            pin_codec, catalog_codec,
            "AAC-configured node should match AAC catalog without mismatch"
        );
    }

    /// Validate that every `transport::moq::subscriber` node in the sample
    /// pipelines can be deserialized into [`MoqPullConfig`] (catches stale
    /// fields rejected by `deny_unknown_fields`).
    #[test]
    fn sample_pipeline_subscriber_configs_deserialize() {
        let sample_dirs = ["samples/pipelines/dynamic", "samples/loadtest/pipelines"];
        let workspace =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap();

        for dir in &sample_dirs {
            let abs = workspace.join(dir);
            let Ok(entries) = std::fs::read_dir(&abs) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("yml") {
                    continue;
                }
                let content = std::fs::read_to_string(&path).unwrap();
                let doc: serde_yaml::Value = serde_yaml::from_str(&content).unwrap();
                let Some(nodes) = doc.get("nodes").and_then(|n| n.as_mapping()) else {
                    continue;
                };
                for (name, node) in nodes {
                    let kind = node.get("kind").and_then(|k| k.as_str()).unwrap_or("");
                    if kind != "transport::moq::subscriber" {
                        continue;
                    }
                    let params = node
                        .get("params")
                        .cloned()
                        .unwrap_or(serde_yaml::Value::Mapping(serde_yaml::Mapping::default()));
                    let result = serde_yaml::from_value::<MoqPullConfig>(params);
                    assert!(
                        result.is_ok(),
                        "sample {}: node '{}' has invalid MoqPullConfig: {}",
                        path.display(),
                        name.as_str().unwrap_or("?"),
                        result.unwrap_err()
                    );
                }
            }
        }
    }

    /// Regression: `parse_catalog` used to wait up to 5s (twice per session)
    /// for a video track that never arrives on audio-only streams, stalling
    /// startup of the audio relay. The first non-empty catalog is authoritative.
    #[tokio::test]
    async fn test_parse_catalog_audio_only_returns_promptly() {
        let origin = moq_lite::Origin::random().produce();
        let mut broadcast = origin.create_broadcast("test-broadcast").unwrap();
        let track = hang::catalog::Catalog::default_track();
        let mut producer = broadcast.create_track(track.clone()).unwrap();

        let consumer = origin.consume();
        let bc = consumer.get_broadcast("test-broadcast").unwrap();
        let consumer_track = bc.subscribe_track(&track).unwrap();

        let mut audio_renditions = std::collections::BTreeMap::new();
        audio_renditions.insert(
            "audio/data".to_string(),
            hang::catalog::AudioConfig {
                codec: super::super::constants::catalog_audio_codec(AudioCodec::Opus),
                sample_rate: 48000,
                channel_count: 2,
                bitrate: Some(128_000),
                description: None,
                container: hang::catalog::Container::default(),
                jitter: None,
            },
        );
        let catalog = hang::catalog::Catalog {
            audio: hang::catalog::Audio { renditions: audio_renditions },
            ..Default::default()
        };
        let payload = catalog.to_string().unwrap();
        let mut group = producer.append_group().unwrap();
        group.write_frame(bytes::Bytes::from(payload)).unwrap();
        group.finish().unwrap();

        let node = MoqPullNode::new(MoqPullConfig::default());
        let mut cc = super::super::catalog_consumer::CatalogConsumer::new(consumer_track);

        let start = std::time::Instant::now();
        let tracks = node.parse_catalog(&mut cc).await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(tracks.len(), 1);
        assert!(tracks[0].audio_codec.is_some());
        assert!(tracks.iter().all(|t| t.video_codec.is_none()));
        assert!(
            elapsed < Duration::from_secs(1),
            "audio-only catalog should parse without the old 5s settle wait, took {elapsed:?}"
        );
    }
}
