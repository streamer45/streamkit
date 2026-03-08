// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Push Node - publishes packets to a MoQ broadcast

use super::constants::DEFAULT_AUDIO_FRAME_DURATION_US;
use async_trait::async_trait;
use opentelemetry::{global, KeyValue};
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketType, VideoCodec,
};
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default)]
pub struct MoqPushConfig {
    pub url: String,
    /// Optional JWT for authenticated MoQ relays. When set, it is appended as `?jwt=...`.
    ///
    /// This is compatible with moq-relay and StreamKit's built-in MoQ auth.
    pub jwt: Option<String>,
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
            jwt: None,
            broadcast: String::new(),
            channels: 2,
            group_duration_ms: default_group_duration_ms(),
            initial_delay_ms: 0,
        }
    }
}

/// A node that receives encoded media and publishes it to a MoQ broadcast.
///
/// Supports both audio (Opus) and video (VP9) inputs. Audio is accepted on
/// the `in` pin, and video on the optional `in_1` pin. When only audio is
/// connected the node behaves identically to its previous audio-only mode.
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
        vec![
            InputPin {
                name: "in".to_string(),
                accepts_types: vec![PacketType::EncodedAudio(EncodedAudioFormat {
                    codec: AudioCodec::Opus,
                    codec_private: None,
                })],
                cardinality: PinCardinality::One,
            },
            InputPin {
                name: "in_1".to_string(),
                accepts_types: vec![PacketType::EncodedVideo(EncodedVideoFormat {
                    codec: VideoCodec::Vp9,
                    bitstream_format: None,
                    codec_private: None,
                    profile: None,
                    level: None,
                })],
                cardinality: PinCardinality::One,
            },
        ]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![] // This is an output node.
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        /// Identifies which input produced the packet.
        enum InputSource {
            Audio,
            Video,
        }

        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let url = match super::parse_moq_url(&self.config.url, self.config.jwt.as_deref()) {
            Ok(url) => url,
            Err(e) => {
                state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
                return Err(e);
            },
        };
        tracing::info!(
            url = %super::redact_url_str_for_logs(&self.config.url),
            broadcast = %self.config.broadcast,
            "MoqPushNode starting"
        );
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

        let publisher_origin = moq_lite::Origin::produce();
        let _publisher_session =
            match client.clone().with_publish(publisher_origin.consume()).connect(url).await {
                Ok(session) => session,
                Err(e) => {
                    let err_msg = format!("Failed to create publisher session: {e}");
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                },
            };

        // Create a transcoded broadcast and publish it
        let mut broadcast =
            publisher_origin.create_broadcast(&self.config.broadcast).ok_or_else(|| {
                StreamKitError::Runtime(format!(
                    "Failed to create broadcast '{}'",
                    self.config.broadcast
                ))
            })?;

        tracing::info!("Publishing to broadcast '{}'", self.config.broadcast);

        // Detect whether video input is connected
        let has_video =
            context.input_types.iter().any(|(_, pt)| matches!(pt, PacketType::EncodedVideo(_)));

        // Create audio track
        let audio_track = moq_lite::Track { name: "audio/data".to_string(), priority: 60 };
        let audio_producer = broadcast
            .create_track(audio_track.clone())
            .map_err(|e| StreamKitError::Runtime(format!("Failed to create audio track: {e}")))?;
        let mut audio_producer: hang::container::OrderedProducer = audio_producer.into();

        // Create video track if video input is connected
        let video_track = if has_video {
            Some(moq_lite::Track { name: "video/data".to_string(), priority: 80 })
        } else {
            None
        };
        let mut video_producer: Option<hang::container::OrderedProducer> =
            if let Some(ref vt) = video_track {
                let producer = broadcast.create_track(vt.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create video track: {e}"))
                })?;
                Some(producer.into())
            } else {
                None
            };

        // Build catalog with both audio and (optionally) video renditions
        let mut audio_renditions = std::collections::BTreeMap::new();
        audio_renditions.insert(
            audio_track.name.clone(),
            hang::catalog::AudioConfig {
                codec: hang::catalog::AudioCodec::Opus,
                sample_rate: 48000,
                channel_count: self.config.channels,
                bitrate: Some(128_000),
                description: None,
                container: hang::catalog::Container::default(),
                jitter: None,
            },
        );

        let mut video_renditions = std::collections::BTreeMap::new();
        if let Some(ref vt) = video_track {
            video_renditions.insert(
                vt.name.clone(),
                hang::catalog::VideoConfig {
                    codec: hang::catalog::VideoCodec::VP9(hang::catalog::VP9 {
                        profile: 0,
                        level: 10,
                        bit_depth: 8,
                        ..hang::catalog::VP9::default()
                    }),
                    coded_width: None,
                    coded_height: None,
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

        // Create catalog track and publish the catalog data
        let mut catalog_producer = broadcast
            .create_track(hang::catalog::Catalog::default_track())
            .map_err(|e| StreamKitError::Runtime(format!("Failed to create catalog track: {e}")))?;
        let catalog_json = match catalog.to_string() {
            Ok(json) => json,
            Err(e) => {
                let err = StreamKitError::Runtime(format!("Failed to serialize catalog: {e}"));
                state_helpers::emit_failed(&context.state_tx, &node_name, err.to_string());
                return Err(err);
            },
        };
        let catalog_data = catalog_json.into_bytes();

        tracing::debug!(
            "publishing catalog JSON: {}",
            std::str::from_utf8(&catalog_data).unwrap_or("<invalid utf8>")
        );

        catalog_producer
            .write_frame(catalog_data)
            .map_err(|e| StreamKitError::Runtime(format!("Failed to write catalog frame: {e}")))?;
        let _catalog_producer = catalog_producer;

        tracing::info!(has_video, "published catalog for broadcast");

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut audio_rx = context.take_input("in")?;
        let mut video_rx = if has_video { Some(context.take_input("in_1")?) } else { None };
        let mut packet_count: u64 = 0;
        let mut audio_clock = MediaClock::new(self.config.initial_delay_ms);
        let mut video_clock = MediaClock::new(self.config.initial_delay_ms);
        let mut audio_seeded = false;
        let mut video_seeded = false;

        // Stats tracking
        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());
        let meter = global::meter("skit_nodes");
        let clock_offset_histogram = meter
            .f64_histogram("moq.push.clock_offset_ms")
            .with_description("Offset between outgoing MoQ timestamp and upstream packet timestamp")
            .with_boundaries(streamkit_core::metrics::HISTOGRAM_BOUNDARIES_CLOCK_OFFSET_MS.to_vec())
            .build();
        let metric_labels = [
            KeyValue::new("node_id", node_name.clone()),
            KeyValue::new("broadcast", self.config.broadcast.clone()),
        ];

        tracing::info!(has_video, "MoqPushNode waiting for input packets...");

        loop {
            let (input_source, packet) = tokio::select! {
                Some(pkt) = audio_rx.recv() => {
                    (InputSource::Audio, pkt)
                },
                Some(pkt) = async {
                    match video_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        _ => std::future::pending().await,
                    }
                } => {
                    (InputSource::Video, pkt)
                },
                Some(control_msg) = context.control_rx.recv() => {
                    if matches!(control_msg, streamkit_core::control::NodeControlMessage::Shutdown) {
                        tracing::info!("MoqPushNode received shutdown signal after {} packets", packet_count);
                        break;
                    }
                    tracing::debug!("MoqPushNode received control message: {:?}", control_msg);
                    continue;
                },
                else => break,
            };

            if let Packet::Binary { data, metadata, .. } = packet {
                let is_first = packet_count == 0;
                packet_count += 1;
                stats_tracker.received();

                if packet_count <= 5 || packet_count.is_multiple_of(50) {
                    tracing::debug!(packet = packet_count, "MoQ publisher sending packet");
                }

                let (clock, seeded, default_dur, producer) = match input_source {
                    InputSource::Audio => (
                        &mut audio_clock,
                        &mut audio_seeded,
                        DEFAULT_AUDIO_FRAME_DURATION_US,
                        &mut audio_producer,
                    ),
                    InputSource::Video => {
                        let Some(vp) = video_producer.as_mut() else {
                            tracing::warn!("video producer missing for video packet");
                            stats_tracker.discarded();
                            continue;
                        };
                        (
                            &mut video_clock,
                            &mut video_seeded,
                            crate::video::DEFAULT_VIDEO_FRAME_DURATION_US,
                            vp,
                        )
                    },
                };

                let duration_us =
                    super::constants::packet_duration_us(metadata.as_ref()).or(Some(default_dur));
                let timestamp_ms =
                    if let Some(meta_ts) = metadata.as_ref().and_then(|m| m.timestamp_us) {
                        if !*seeded {
                            clock.seed_from_timestamp_us(meta_ts);
                            *seeded = true;
                        }
                        meta_ts.saturating_add(999) / 1_000 + self.config.initial_delay_ms
                    } else {
                        clock.timestamp_ms()
                    };

                let keyframe = match input_source {
                    InputSource::Audio => {
                        is_first || clock.is_group_boundary_ms(self.config.group_duration_ms)
                    },
                    InputSource::Video => {
                        metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(false)
                    },
                };

                let timestamp =
                    hang::container::Timestamp::from_millis(timestamp_ms).map_err(|_| {
                        StreamKitError::Runtime("MoQ frame timestamp overflow".to_string())
                    })?;

                let mut payload = hang::container::BufList::new();
                payload.push_chunk(data);

                if keyframe {
                    if let Err(e) = producer.keyframe() {
                        let err_msg = format!("Failed to signal keyframe: {e}");
                        tracing::warn!("{err_msg}");
                        stats_tracker.errored();
                        stats_tracker.force_send();
                        state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                        return Err(StreamKitError::Runtime(err_msg));
                    }
                }

                let frame = hang::container::Frame { timestamp, payload };

                if let Err(e) = producer.write(frame) {
                    let err_msg = format!("Failed to write MoQ frame: {e}");
                    tracing::warn!("{err_msg}");
                    stats_tracker.errored();
                    stats_tracker.force_send();
                    state_helpers::emit_failed(&context.state_tx, &node_name, &err_msg);
                    return Err(StreamKitError::Runtime(err_msg));
                }

                if let Some(meta_ts) = metadata.as_ref().and_then(|m| m.timestamp_us) {
                    let meta_ms = meta_ts / 1_000;
                    let offset = timestamp_ms.saturating_sub(meta_ms);
                    #[allow(clippy::cast_precision_loss)]
                    {
                        clock_offset_histogram.record(offset as f64, &metric_labels);
                    }
                }

                clock.advance_by_duration_us(duration_us, default_dur);
                stats_tracker.sent();
            } else {
                tracing::warn!("MoqPushNode received non-binary packet, ignoring");
                stats_tracker.discarded();
            }

            stats_tracker.maybe_send();
        }

        tracing::info!(
            "MoqPushNode input channels closed after {} packets - pipeline upstream ended",
            packet_count
        );

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        // Close tracks when done (best-effort)
        let _ = audio_producer.track.finish();
        if let Some(mut vp) = video_producer {
            let _ = vp.track.finish();
        }

        tracing::info!("MoqPushNode finished after sending {} packets", packet_count);
        Ok(())
    }
}
