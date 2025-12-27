// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Push Node - publishes packets to a MoQ broadcast

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    packet_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct MoqPushConfig {
    pub url: String,
    pub broadcast: String,
    #[serde(default = "default_channels")]
    pub channels: u32,
    /// Duration of each MoQ group in milliseconds.
    /// Smaller groups = lower latency but more overhead.
    /// Larger groups = higher latency but better efficiency.
    /// Default: 40ms (2 Opus frames at 20ms each).
    /// For real-time applications, use 20-60ms. For high-latency networks, use 100ms+.
    #[serde(default = "default_group_duration_ms")]
    pub group_duration_ms: u64,
    /// Adds a timestamp offset (playout delay) so receivers can buffer before playback.
    ///
    /// This is especially helpful when subscribers are on higher-latency / higher-jitter links,
    /// and the client begins playback as soon as it sees the first frame.
    ///
    /// Default: 0 (no added delay).
    pub initial_delay_ms: u64,
}

const fn default_channels() -> u32 {
    2 // Stereo by default for backwards compatibility
}

const fn default_group_duration_ms() -> u64 {
    40 // 2 Opus frames for low latency
}

impl Default for MoqPushConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            broadcast: String::new(),
            channels: 2,
            group_duration_ms: default_group_duration_ms(),
            initial_delay_ms: 0,
        }
    }
}

/// A node that receives Opus packets and publishes them to a MoQ broadcast.
pub struct MoqPushNode {
    config: MoqPushConfig,
}

impl MoqPushNode {
    pub const fn new(config: MoqPushConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl ProcessorNode for MoqPushNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::OpusAudio],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![] // This is an output node.
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let url = match self.config.url.parse() {
            Ok(url) => url,
            Err(e) => {
                let err_msg = format!("Failed to parse MoQ URL '{}': {}", self.config.url, e);
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(StreamKitError::Configuration(err_msg));
            },
        };
        tracing::info!(url = %self.config.url, broadcast = %self.config.broadcast, "MoqPushNode starting");
        tracing::info!(
            group_duration_ms = self.config.group_duration_ms,
            initial_delay_ms = self.config.initial_delay_ms,
            "MoqPushNode timing configuration"
        );

        let client = match super::shared_insecure_client() {
            Ok(c) => c,
            Err(e) => {
                let err_msg = format!("{e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(e);
            },
        };

        let publisher_session = match client.connect(url).await {
            Ok(s) => s,
            Err(e) => {
                let err_msg = format!("Failed to connect: {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(StreamKitError::Runtime(err_msg));
            },
        };

        let publisher_origin = moq_lite::Origin::produce();
        if let Err(e) =
            moq_lite::Session::connect(publisher_session, publisher_origin.consumer, None).await
        {
            let err_msg = format!("Failed to create publisher session: {e}");
            state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
            return Err(StreamKitError::Runtime(err_msg));
        }

        // Create a transcoded broadcast and publish it
        let transcoded_broadcast = moq_lite::Broadcast::produce();

        // Publish the transcoded broadcast via the publisher session
        publisher_origin
            .producer
            .publish_broadcast(&self.config.broadcast, transcoded_broadcast.consumer);

        let mut broadcast = transcoded_broadcast.producer;

        tracing::info!("Publishing to broadcast '{}'", self.config.broadcast);

        // Create an audio track for the opus data.
        // Match @moq/hang defaults for interoperability.
        let audio_track = moq_lite::Track { name: "audio/data".to_string(), priority: 80 };

        let track_producer = broadcast.create_track(audio_track.clone());
        let mut track_producer: hang::TrackProducer = track_producer.into();

        // Create and publish a catalog describing our audio track
        let mut audio_renditions = std::collections::BTreeMap::new();
        audio_renditions.insert(
            audio_track.name.clone(),
            hang::catalog::AudioConfig {
                codec: hang::catalog::AudioCodec::Opus,
                sample_rate: 48000,                  // Default opus sample rate
                channel_count: self.config.channels, // From configuration
                bitrate: Some(128_000),              // Default bitrate
                description: None,
            },
        );

        let catalog = hang::catalog::Catalog {
            audio: Some(hang::catalog::Audio { renditions: audio_renditions, priority: 80 }),
            ..Default::default()
        };

        // Create catalog track and publish the catalog data
        let mut catalog_producer = broadcast.create_track(hang::catalog::Catalog::default_track());
        let catalog_json = match catalog.to_string() {
            Ok(json) => json,
            Err(e) => {
                let err_msg = format!("Failed to serialize catalog: {e}");
                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                return Err(StreamKitError::Runtime(err_msg));
            },
        };
        let catalog_data = catalog_json.into_bytes(); // Avoid intermediate Vec allocation

        tracing::debug!(
            "publishing catalog JSON: {}",
            std::str::from_utf8(&catalog_data).unwrap_or("<invalid utf8>")
        );

        // Write the catalog frame
        catalog_producer.write_frame(catalog_data);
        // Keep the catalog track producer alive for the lifetime of the broadcast.
        // If dropped, the underlying moq-lite track gets cancelled and watchers will go "offline".
        let _catalog_producer = catalog_producer;

        tracing::info!("published catalog for broadcast");

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut input_rx = context.take_input("in")?;
        let mut packet_count: u64 = 0;
        let mut clock = super::constants::MediaClock::new(self.config.initial_delay_ms);

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Read opus packets and write them to the MoQ track
        tracing::info!("MoqPushNode waiting for input packets...");
        loop {
            tokio::select! {
                Some(first_packet) = input_rx.recv() => {
                    // Greedily collect a batch of packets
                    let packet_batch = packet_helpers::batch_packets_greedy(
                        first_packet,
                        &mut input_rx,
                        context.batch_size,
                    );

                    for packet in packet_batch {
                        if let Packet::Binary { data, metadata, .. } = packet {
                            let is_first = packet_count == 0;
                            packet_count += 1;
                            stats_tracker.received();

                            if packet_count <= 5 || packet_count.is_multiple_of(50) {
                                tracing::debug!(packet = packet_count, "MoQ publisher sending packet");
                            }

                            let duration_us = super::constants::packet_duration_us(metadata.as_ref());
                            let timestamp_ms = clock.timestamp_ms();
                            let keyframe =
                                is_first || clock.is_group_boundary(self.config.group_duration_ms);

                            let timestamp = hang::Timestamp::from_millis(timestamp_ms).map_err(|_| {
                                StreamKitError::Runtime("MoQ frame timestamp overflow".to_string())
                            })?;

                            let mut payload = hang::BufList::new();
                            payload.push_chunk(data);

                            let frame = hang::Frame { timestamp, keyframe, payload };

                            if let Err(e) = track_producer.write(frame) {
                                let err_msg = format!("Failed to write MoQ frame: {e}");
                                tracing::warn!("{err_msg}");
                                state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                                return Err(StreamKitError::Runtime(err_msg));
                            }

                            clock.advance_by_duration_us(duration_us);
                            stats_tracker.sent();
                        } else {
                            tracing::warn!("MoqPushNode received non-binary packet, ignoring");
                            stats_tracker.discarded();
                        }
                    }
                    stats_tracker.maybe_send();
                },
                Some(control_msg) = context.control_rx.recv() => {
                    match control_msg {
                        streamkit_core::control::NodeControlMessage::Shutdown => {
                            tracing::info!("MoqPushNode received shutdown signal after {} packets", packet_count);
                            break;
                        }
                        _ => {
                            tracing::debug!("MoqPushNode received control message: {:?}", control_msg);
                        }
                    }
                },
                else => break
            }
        }
        tracing::info!(
            "MoqPushNode input channel closed after {} packets - pipeline upstream ended",
            packet_count
        );

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        // Close the track when done (best-effort)
        track_producer.inner.clone().close();

        tracing::info!("MoqPushNode finished after sending {} packets", packet_count);
        Ok(())
    }
}
