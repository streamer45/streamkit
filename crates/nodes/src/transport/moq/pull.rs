// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Pull Node - subscribes to broadcasts from a MoQ server

use async_trait::async_trait;
use bytes::Buf;
use moq_lite::coding::Decode;
use moq_lite::AsPath;
use schemars::JsonSchema;
use serde::Deserialize;
use std::time::Duration;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};

#[derive(Deserialize, Debug, JsonSchema, Clone, Default)]
#[serde(default)]
pub struct MoqPullConfig {
    pub url: String,
    pub broadcast: String,
    /// Batch window in milliseconds. If > 0, after receiving a frame the node will
    /// wait up to this duration to collect additional frames before forwarding.
    /// Default: 0 (no batching) - recommended because moq_lite's TrackConsumer::read()
    /// has internal allocation overhead that makes batching counterproductive.
    pub batch_ms: u64,
}

/// A node that connects to a MoQ server, subscribes to a broadcast,
/// and outputs the received media as Opus packets.
///
/// This node performs catalog discovery during initialization.
///
/// **Output pins**
/// - Always exposes a stable `out` pin for backward-compatible pipelines.
/// - Also exposes one output pin per discovered Opus track (by track name).
/// - At runtime, the node currently subscribes to the first discovered Opus track and emits
///   its packets to both `out` and the track-named pin.
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
                produces_type: PacketType::OpusAudio,
                cardinality: PinCardinality::Broadcast,
            }],
        }
    }

    fn stable_out_pin() -> OutputPin {
        OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::OpusAudio,
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
            pins.push(OutputPin {
                name: track.name.clone(),
                produces_type: PacketType::OpusAudio,
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
            url = %self.config.url,
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
        tracing::info!(url = %self.config.url, broadcast = %self.config.broadcast, "MoqPullNode starting");
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
    ) -> Result<bytes::Bytes, moq_lite::Error> {
        // hang protocol: frame payload is prefixed with a varint u64 timestamp in microseconds.
        // We discard it here and forward the remaining bytes (Opus frame data).
        let _timestamp_micros = u64::decode(&mut payload, moq_lite::lite::Version::Draft02)?;
        Ok(payload.copy_to_bytes(payload.remaining()))
    }

    async fn read_next_raw_moq(
        track_consumer: &mut moq_lite::TrackConsumer,
        current_group: &mut Option<moq_lite::GroupConsumer>,
    ) -> Result<Option<bytes::Bytes>, moq_lite::Error> {
        loop {
            if current_group.is_none() {
                match track_consumer.next_group().await {
                    Ok(Some(group)) => *current_group = Some(group),
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
            url = %self.config.url,
            broadcast = %self.config.broadcast,
            "Connecting to MoQ server to discover tracks"
        );

        let url = self.config.url.parse().map_err(|e| {
            StreamKitError::Configuration(format!(
                "Failed to parse MoQ URL '{}': {}",
                self.config.url, e
            ))
        })?;

        let client = super::shared_insecure_client()?;

        let session = client
            .connect(url)
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Failed to connect: {e}")))?;

        let origin = moq_lite::Origin::produce();
        let _consumer_session =
            moq_lite::Session::connect(session, None, origin.producer).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Subscribe to the specified broadcast.
        //
        // During dynamic session initialization, the broadcast may not have been announced yet.
        // Treat this as "no tracks discovered" rather than a hard error: the runtime `run()` path
        // already waits for announcements and will connect once the broadcast appears.
        let Some(broadcast) = origin.consumer.consume_broadcast(&self.config.broadcast) else {
            tracing::debug!(
                broadcast = %self.config.broadcast,
                "Broadcast not available during catalog discovery; using default output pin"
            );
            return Ok(Vec::new());
        };

        // Subscribe to the catalog track
        let raw_catalog_track = broadcast.subscribe_track(&hang::catalog::Catalog::default_track());
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

            if let Some(audio) = catalog.audio {
                for (track_name, config) in audio.renditions {
                    match config.codec {
                        hang::catalog::AudioCodec::Opus => {
                            tracing::info!(track = %track_name, "found opus audio track");
                            let track =
                                moq_lite::Track { name: track_name, priority: audio.priority };
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
            }

            if !tracks.is_empty() {
                return Ok(tracks);
            }

            // Check if we've exceeded the overall timeout
            if start.elapsed() >= CATALOG_TIMEOUT {
                return Err(StreamKitError::Runtime(format!(
                    "No opus audio tracks found in catalog after {} seconds",
                    CATALOG_TIMEOUT.as_secs()
                )));
            }

            // Catalog is empty, wait a bit before checking for the next update
            tracing::trace!("Catalog has no opus tracks yet, waiting for next update...");
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }

    // MoQ connection state machine with multiplexed track handling and error recovery
    // High complexity is inherent to protocol handling (track management, object streaming, packet routing)
    #[allow(clippy::cognitive_complexity)]
    async fn run_connection(
        &self,
        context: &mut NodeContext,
        total_packet_count: &mut u32,
    ) -> Result<StreamEndReason, StreamKitError> {
        let url = self.config.url.parse().map_err(|e| {
            StreamKitError::Configuration(format!(
                "Failed to parse MoQ URL '{}': {}",
                self.config.url, e
            ))
        })?;

        let client = super::shared_insecure_client()?;

        let session = client
            .connect(url)
            .await
            .map_err(|e| StreamKitError::Runtime(format!("Failed to connect: {e}")))?;

        // Create origin for consuming broadcasts only (no publishing to avoid cycles)
        let origin = moq_lite::Origin::produce();
        let _consumer_session =
            moq_lite::Session::connect(session, None, origin.producer).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Wait for broadcast to become available
        // Note: consume_broadcast() only works after announcement, so we primarily rely on announcements
        let broadcast = {
            let mut consumer = origin.consumer.clone();

            // Try immediate consume first (works if broadcast already announced)
            if let Some(broadcast) = origin.consumer.consume_broadcast(&self.config.broadcast) {
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
                            Some((path, maybe_broadcast)) = consumer.announced() => {
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

        // First, get the catalog to find audio tracks
        let raw_catalog_track = broadcast.subscribe_track(&hang::catalog::Catalog::default_track());
        let mut catalog_consumer = hang::catalog::CatalogConsumer::new(raw_catalog_track);

        tracing::debug!(
            "subscribed to catalog track: {}",
            hang::catalog::Catalog::default_track().name
        );

        // Wait for catalog data with timeout
        let audio_tracks = self.parse_catalog(&mut catalog_consumer).await?;

        if audio_tracks.is_empty() {
            return Err(StreamKitError::Runtime(
                "No opus audio tracks found in broadcast".to_string(),
            ));
        }

        // Subscribe to the first opus audio track
        let audio_track = &audio_tracks[0];
        tracing::info!("subscribing to audio track: {}", audio_track.name);
        let track_pin_name = audio_track.name.as_str();

        // Determine once if track pin is registered (stable for the connection)
        let track_pin_registered = self.output_pins.iter().any(|p| p.name == track_pin_name);

        // Use moq_lite's TrackConsumer directly.
        //
        // hang::TrackConsumer (hang v0.9.1) can enter a tight CPU loop when monitoring pending
        // groups (see hang::model::group::GroupConsumer::buffer_until rotating buffered frames).
        // In practice this can stall audio after some time and prevent clean shutdown.
        //
        // For audio we prefer low-latency, "latest group" semantics: we always read the latest
        // announced group and drain it, letting moq_lite drop old groups if we're slow.
        let mut track_consumer = broadcast.subscribe_track(audio_track);
        let mut current_group: Option<moq_lite::GroupConsumer> = None;

        let mut session_packet_count: u32 = 0;

        // Stats tracking
        let node_name = context.output_sender.node_name().to_string();
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Read audio frames directly using async calls
        tracing::info!("starting to read audio frames from track: {}", audio_track.name);

        loop {
            // Block waiting for the first frame of a potential batch, with cancellation and control message support.
            let read_result: Result<Option<bytes::Bytes>, moq_lite::Error> = if let Some(token) =
                &context.cancellation_token
            {
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
                    result = Self::read_next_raw_moq(&mut track_consumer, &mut current_group) => result,
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
                    result = Self::read_next_raw_moq(&mut track_consumer, &mut current_group) => result,
                }
            };

            match read_result {
                Ok(Some(first_payload)) => {
                    // Batching is disabled by default (batch_ms=0).
                    if self.config.batch_ms > 0 {
                        let mut batch = Vec::with_capacity(context.batch_size);
                        batch.push(first_payload);

                        let batch_deadline = tokio::time::Instant::now()
                            + std::time::Duration::from_millis(self.config.batch_ms);

                        while batch.len() < context.batch_size {
                            let time_remaining = batch_deadline
                                .saturating_duration_since(tokio::time::Instant::now());
                            if time_remaining.is_zero() {
                                break;
                            }

                            match tokio::time::timeout(
                                time_remaining,
                                Self::read_next_raw_moq(&mut track_consumer, &mut current_group),
                            )
                            .await
                            {
                                Ok(Ok(Some(payload))) => batch.push(payload),
                                _ => break,
                            }
                        }

                        for payload in batch {
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

                            let data = match Self::strip_hang_timestamp_header(payload) {
                                Ok(data) => data,
                                Err(e) => {
                                    tracing::warn!("Failed to decode frame timestamp: {e}");
                                    stats_tracker.discarded();
                                    continue;
                                },
                            };
                            let packet =
                                Packet::Binary { data, content_type: None, metadata: None };

                            if track_pin_registered
                                && track_pin_name != "out"
                                && context
                                    .output_sender
                                    .send(track_pin_name, packet.clone())
                                    .await
                                    .is_err()
                            {
                                tracing::debug!("Output channel closed, stopping node");
                                return Ok(StreamEndReason::Natural);
                            }
                            if context.output_sender.send("out", packet).await.is_err() {
                                tracing::debug!("Output channel closed, stopping node");
                                return Ok(StreamEndReason::Natural);
                            }
                            stats_tracker.sent();
                        }
                    } else {
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

                        let data = match Self::strip_hang_timestamp_header(first_payload) {
                            Ok(data) => data,
                            Err(e) => {
                                tracing::warn!("Failed to decode frame timestamp: {e}");
                                stats_tracker.discarded();
                                continue;
                            },
                        };

                        let packet = Packet::Binary { data, content_type: None, metadata: None };
                        if track_pin_registered
                            && track_pin_name != "out"
                            && context
                                .output_sender
                                .send(track_pin_name, packet.clone())
                                .await
                                .is_err()
                        {
                            tracing::debug!("Output channel closed, stopping node");
                            return Ok(StreamEndReason::Natural);
                        }
                        if context.output_sender.send("out", packet).await.is_err() {
                            tracing::debug!("Output channel closed, stopping node");
                            return Ok(StreamEndReason::Natural);
                        }
                        stats_tracker.sent();
                    }

                    stats_tracker.maybe_send();
                },
                Ok(None) => {
                    tracing::info!(
                        "Track stream ended naturally after {} packets",
                        session_packet_count
                    );
                    return Ok(StreamEndReason::Natural);
                },
                Err(moq_lite::Error::Cancel) => {
                    tracing::debug!(
                        session_packet_count,
                        total_packet_count = *total_packet_count,
                        "Track read cancelled"
                    );
                    return Ok(StreamEndReason::Reconnect);
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
mod tests {
    use super::*;
    use bytes::BytesMut;
    use moq_lite::coding::Encode;

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
        123_u64.encode(&mut buf, moq_lite::lite::Version::Draft02);
        buf.extend_from_slice(b"opus-frame-bytes");
        let payload = buf.freeze();

        let stripped = match MoqPullNode::strip_hang_timestamp_header(payload) {
            Ok(stripped) => stripped,
            Err(e) => panic!("decode failed: {e}"),
        };
        assert_eq!(&stripped[..], b"opus-frame-bytes");
    }
}
