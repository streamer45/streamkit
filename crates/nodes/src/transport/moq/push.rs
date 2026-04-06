// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! MoQ Push Node - publishes packets to a MoQ broadcast

use super::constants::{
    catalog_audio_codec, catalog_video_codec, default_audio_frame_duration_us,
    moq_accepted_media_types, resolve_audio_codec, resolve_video_codec,
};
use async_trait::async_trait;
use futures::future::poll_fn;
use opentelemetry::{global, KeyValue};
use schemars::JsonSchema;
use serde::Deserialize;
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{AudioCodec, Packet, PacketType, VideoCodec};
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
    /// Whether to publish an audio track (Opus on the `in` pin).
    ///
    /// Required for dynamic pipelines where `input_types` is not available at
    /// startup. In oneshot pipelines this is auto-detected from `input_types`
    /// when left as `None`.
    pub audio: Option<bool>,
    /// Whether to publish a video track (VP9/AV1 on the `in_1` pin).
    ///
    /// Required for dynamic pipelines where `input_types` is not available at
    /// startup. In oneshot pipelines this is auto-detected from `input_types`
    /// when left as `None`.
    pub video: Option<bool>,
    /// Video codec for the MoQ catalog.
    ///
    /// Required for dynamic pipelines where `input_types` is not available at
    /// startup.  Accepted values: `"vp9"`, `"av1"`.  When `None`, the codec
    /// is auto-detected from `input_types` (static pipelines) and falls back
    /// to VP9.
    pub video_codec: Option<String>,
    /// Audio codec for the MoQ catalog.
    ///
    /// Required for dynamic pipelines where `input_types` is not available at
    /// startup.  Accepted values: `"opus"`, `"aac"`.  When `None`, the codec
    /// is auto-detected from `input_types` (static pipelines) and falls back
    /// to Opus.
    pub audio_codec: Option<String>,
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
            audio: None,
            video: None,
            video_codec: None,
            audio_codec: None,
            group_duration_ms: default_group_duration_ms(),
            initial_delay_ms: 0,
        }
    }
}

/// A node that receives encoded media and publishes it to a MoQ broadcast.
///
/// Supports arbitrary combinations of audio (Opus) and video (VP9) inputs.
/// Audio is accepted on the `in` pin, and video on the `in_1` pin.
/// Either or both may be connected; at least one must be present.
pub struct MoqPushNode {
    config: MoqPushConfig,
}

/// State for a single dynamic input pin.
struct DynamicInputState {
    pin_name: String,
    receiver: tokio::sync::mpsc::Receiver<Packet>,
    producer: hang::container::OrderedProducer,
    clock: MediaClock,
    seeded: bool,
    first_sent: bool,
    is_video: bool,
}

impl MoqPushNode {
    pub const fn new(config: MoqPushConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl ProcessorNode for MoqPushNode {
    fn input_pins(&self) -> Vec<InputPin> {
        let accepted_types = moq_accepted_media_types();
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
        vec![] // This is an output node.
    }

    fn supports_dynamic_pins(&self) -> bool {
        true
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        /// Identifies which input produced the packet.
        enum InputSource {
            Audio,
            Video,
            /// A dynamic input pin, identified by index into `dynamic_inputs`.
            Dynamic(usize),
        }

        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        let mut url = match super::parse_moq_url(&self.config.url, self.config.jwt.as_deref()) {
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

        // Pre-resolve hostname to avoid QUIC IPv6 timeout (see resolve_url_for_quic docs)
        if let Err(e) = super::resolve_url_for_quic(&mut url).await {
            tracing::warn!(error = %e, "Failed to pre-resolve MoQ URL; proceeding with original");
        }

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

        // Detect which inputs are connected.
        // Explicit config (`audio`/`video`) takes priority — required for dynamic
        // pipelines where `input_types` is empty at startup.
        // Falls back to `input_types` (populated by the graph builder in oneshot pipelines).
        let has_audio = self.config.audio.unwrap_or_else(|| {
            context.input_types.iter().any(|(_, pt)| matches!(pt, PacketType::EncodedAudio(_)))
        });
        let has_video = self.config.video.unwrap_or_else(|| {
            context.input_types.iter().any(|(_, pt)| matches!(pt, PacketType::EncodedVideo(_)))
        });

        // Detect the upstream video codec so the catalog reflects the actual
        // encoding.  Priority order:
        // 1. Explicit `video_codec` config param (required for dynamic pipelines)
        // 2. Auto-detected from `input_types` (static pipelines)
        // 3. Default: VP9
        let video_codec =
            resolve_video_codec(self.config.video_codec.as_deref(), &context.input_types);

        // Detect the upstream audio codec (same pattern as video).
        let audio_codec =
            resolve_audio_codec(self.config.audio_codec.as_deref(), &context.input_types);

        if !has_audio && !has_video {
            let err_msg = "MoqPushNode requires at least one audio or video input";
            state_helpers::emit_failed(&context.state_tx, &node_name, err_msg);
            return Err(StreamKitError::Configuration(err_msg.to_string()));
        }

        // Create audio track if audio input is connected
        let audio_track = if has_audio {
            Some(moq_lite::Track { name: "audio/data".to_string(), priority: 80 })
        } else {
            None
        };
        let mut audio_producer: Option<hang::container::OrderedProducer> =
            if let Some(ref at) = audio_track {
                let producer = broadcast.create_track(at.clone()).map_err(|e| {
                    StreamKitError::Runtime(format!("Failed to create audio track: {e}"))
                })?;
                Some(producer.into())
            } else {
                None
            };

        // Create video track if video input is connected
        let video_track = if has_video {
            Some(moq_lite::Track { name: "video/data".to_string(), priority: 60 })
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

        // Build catalog with connected renditions
        let mut audio_renditions = std::collections::BTreeMap::new();
        if let Some(ref at) = audio_track {
            audio_renditions.insert(
                at.name.clone(),
                hang::catalog::AudioConfig {
                    codec: catalog_audio_codec(audio_codec),
                    sample_rate: 48000,
                    channel_count: self.config.channels,
                    bitrate: Some(128_000),
                    description: None,
                    container: hang::catalog::Container::default(),
                    jitter: None,
                },
            );
        }

        let mut video_renditions = std::collections::BTreeMap::new();
        if let Some(ref vt) = video_track {
            video_renditions.insert(
                vt.name.clone(),
                hang::catalog::VideoConfig {
                    codec: catalog_video_codec(video_codec),
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

        let mut catalog = hang::catalog::Catalog {
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
        // Keep catalog producer and state alive so we can re-publish when
        // dynamic tracks are added at runtime.

        tracing::info!(has_video, "published catalog for broadcast");

        state_helpers::emit_running(&context.state_tx, &node_name);

        let mut audio_rx = if has_audio { Some(context.take_input("in")?) } else { None };
        let mut video_rx = if has_video { Some(context.take_input("in_1")?) } else { None };
        let mut packet_count: u64 = 0;
        let mut audio_clock = MediaClock::new(self.config.initial_delay_ms);
        let mut video_clock = MediaClock::new(self.config.initial_delay_ms);
        let mut audio_seeded = false;
        let mut video_seeded = false;
        let mut audio_first_sent = false;
        let mut video_first_sent = false;

        // Pin management for dynamic input pins
        let mut pin_mgmt_rx = context.pin_management_rx.take();

        // Dynamic input state: used for runtime-added input pins beyond the static `in`/`in_1`.
        let mut dynamic_inputs: Vec<DynamicInputState> = Vec::new();

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
                pkt = async {
                    match audio_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        _ => std::future::pending().await,
                    }
                } => {
                    if let Some(p) = pkt {
                        (InputSource::Audio, p)
                    } else {
                        audio_rx = None;
                        if video_rx.is_none() && dynamic_inputs.is_empty() { break; }
                        continue;
                    }
                },
                pkt = async {
                    match video_rx.as_mut() {
                        Some(rx) => rx.recv().await,
                        _ => std::future::pending().await,
                    }
                } => {
                    if let Some(p) = pkt {
                        (InputSource::Video, p)
                    } else {
                        video_rx = None;
                        if audio_rx.is_none() && dynamic_inputs.is_empty() { break; }
                        continue;
                    }
                },
                // Handle dynamic input pin management messages
                Some(msg) = async {
                    match &mut pin_mgmt_rx {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    Self::handle_pin_management(
                        msg,
                        &mut broadcast,
                        &mut dynamic_inputs,
                        self.config.initial_delay_ms,
                        &mut catalog,
                        &mut catalog_producer,
                        self.config.channels,
                        video_codec,
                        audio_codec,
                    );
                    continue;
                },
                // Poll all dynamic input pin receivers.
                // NOTE: iteration always starts from index 0, so under sustained
                // load the first ready receiver wins. This is an accepted
                // trade-off for simplicity — in practice dynamic inputs carry
                // independent media tracks at moderate frame rates, making
                // starvation unlikely.
                //
                // Safety w.r.t. select!: the poll_fn closure borrows
                // `&mut dynamic_inputs`, and the pin-management branch also
                // mutates it. This is safe because select! drops the losing
                // future *before* executing the winning branch, so the mutable
                // borrow from poll_fn is released before pin-management runs.
                result = async {
                    if dynamic_inputs.is_empty() {
                        return std::future::pending().await;
                    }
                    poll_fn(|cx| {
                        for (idx, state) in dynamic_inputs.iter_mut().enumerate() {
                            match state.receiver.poll_recv(cx) {
                                std::task::Poll::Ready(result) => {
                                    return std::task::Poll::Ready((idx, result));
                                },
                                std::task::Poll::Pending => {},
                            }
                        }
                        std::task::Poll::Pending
                    }).await
                } => {
                    let (idx, maybe_pkt) = result;
                    if let Some(p) = maybe_pkt {
                        (InputSource::Dynamic(idx), p)
                    } else {
                        // Dynamic receiver closed — remove it (swap_remove is
                        // O(1); order doesn't matter since indices are recomputed
                        // each poll iteration).
                        let mut removed = dynamic_inputs.swap_remove(idx);
                        tracing::info!(pin = %removed.pin_name, "Dynamic input pin closed");
                        let _ = removed.producer.track.finish();
                        if audio_rx.is_none() && video_rx.is_none() && dynamic_inputs.is_empty() { break; }
                        continue;
                    }
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
                packet_count += 1;
                stats_tracker.received();

                if packet_count <= 5 || packet_count.is_multiple_of(50) {
                    tracing::debug!(packet = packet_count, "MoQ publisher sending packet");
                }

                // Determine which clock/producer/flags to use based on input source
                let is_video_source;
                let (clock, seeded, default_dur, first_sent, producer) = match input_source {
                    InputSource::Audio => {
                        is_video_source = false;
                        let Some(ap) = audio_producer.as_mut() else {
                            tracing::warn!("audio producer missing for audio packet");
                            stats_tracker.discarded();
                            continue;
                        };
                        (
                            &mut audio_clock,
                            &mut audio_seeded,
                            default_audio_frame_duration_us(audio_codec),
                            &mut audio_first_sent,
                            ap,
                        )
                    },
                    InputSource::Video => {
                        is_video_source = true;
                        let Some(vp) = video_producer.as_mut() else {
                            tracing::warn!("video producer missing for video packet");
                            stats_tracker.discarded();
                            continue;
                        };
                        (
                            &mut video_clock,
                            &mut video_seeded,
                            crate::video::DEFAULT_VIDEO_FRAME_DURATION_US,
                            &mut video_first_sent,
                            vp,
                        )
                    },
                    InputSource::Dynamic(idx) => {
                        let state = &mut dynamic_inputs[idx];
                        is_video_source = state.is_video;
                        let dur = if state.is_video {
                            crate::video::DEFAULT_VIDEO_FRAME_DURATION_US
                        } else {
                            default_audio_frame_duration_us(audio_codec)
                        };
                        (
                            &mut state.clock,
                            &mut state.seeded,
                            dur,
                            &mut state.first_sent,
                            &mut state.producer,
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

                let keyframe = if is_video_source {
                    // Default to true when keyframe metadata is missing to ensure
                    // the OrderedProducer opens an initial MoQ group.
                    metadata.as_ref().and_then(|m| m.keyframe).unwrap_or(true)
                } else {
                    let first = !*first_sent;
                    *first_sent = true;
                    first || clock.is_group_boundary_ms(self.config.group_duration_ms)
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
        if let Some(mut ap) = audio_producer {
            let _ = ap.track.finish();
        }
        if let Some(mut vp) = video_producer {
            let _ = vp.track.finish();
        }
        for mut state in dynamic_inputs {
            let _ = state.producer.track.finish();
        }

        tracing::info!("MoqPushNode finished after sending {} packets", packet_count);
        Ok(())
    }
}

/// Derive the MoQ track name from a pin name.
///
/// Names starting with `"video/"` or `"audio/"` are used as-is (they already
/// follow the hang protocol convention).  Bare names without a prefix default
/// to audio tracks by prepending `"audio/"`.
fn track_name_from_pin(pin_name: &str) -> String {
    if pin_name.starts_with("video/") || pin_name.starts_with("audio/") {
        pin_name.to_string()
    } else {
        format!("audio/{pin_name}")
    }
}

/// Return `true` if a pin name denotes a video track (i.e. starts with `"video/"`).
fn is_video_pin(pin_name: &str) -> bool {
    pin_name.starts_with("video/")
}

impl MoqPushNode {
    /// Handle pin management messages for dynamic input pins.
    ///
    /// When a new dynamic input pin is added, creates a corresponding MoQ track,
    /// registers the receiver so that packets from that pin get published, and
    /// re-publishes the catalog so downstream MoQ subscribers can discover it.
    #[allow(clippy::too_many_arguments)]
    fn handle_pin_management(
        msg: PinManagementMessage,
        broadcast: &mut moq_lite::BroadcastProducer,
        dynamic_inputs: &mut Vec<DynamicInputState>,
        initial_delay_ms: u64,
        catalog: &mut hang::catalog::Catalog,
        catalog_producer: &mut moq_lite::TrackProducer,
        channels: u32,
        video_codec: VideoCodec,
        audio_codec: AudioCodec,
    ) {
        match msg {
            PinManagementMessage::RequestAddInputPin { suggested_name, response_tx } => {
                let pin_name = suggested_name.unwrap_or_else(|| "dynamic_in".to_string());
                tracing::info!("MoqPushNode: creating dynamic input pin '{}'", pin_name);
                let pin = InputPin {
                    name: pin_name,
                    accepts_types: moq_accepted_media_types(),
                    cardinality: PinCardinality::One,
                };
                let _ = response_tx.send(Ok(pin));
            },
            PinManagementMessage::AddedInputPin { pin, channel, hint_tx: _ } => {
                Self::activate_dynamic_input(
                    &pin,
                    channel,
                    broadcast,
                    dynamic_inputs,
                    initial_delay_ms,
                    catalog,
                    catalog_producer,
                    channels,
                    video_codec,
                    audio_codec,
                );
            },
            PinManagementMessage::RemoveInputPin { pin_name } => {
                tracing::info!("MoqPushNode: removed input pin '{}'", pin_name);
                let track_name = track_name_from_pin(&pin_name);
                // Remove the matching entry — swap_remove is O(1) and order
                // doesn't matter since indices are recomputed each poll iteration.
                if let Some(pos) = dynamic_inputs.iter().position(|s| s.pin_name == pin_name) {
                    let mut removed = dynamic_inputs.swap_remove(pos);
                    let _ = removed.producer.track.finish();
                }
                // Remove the track from the catalog and re-publish.
                Self::remove_catalog_rendition(catalog, &track_name);
                Self::republish_catalog(catalog, catalog_producer);
            },
            PinManagementMessage::RequestAddOutputPin { response_tx, .. } => {
                let _ = response_tx.send(Err(StreamKitError::Configuration(
                    "MoqPushNode does not support dynamic output pins".to_string(),
                )));
            },
            PinManagementMessage::AddedOutputPin { .. }
            | PinManagementMessage::RemoveOutputPin { .. }
            | PinManagementMessage::InputTypeResolved { .. }
            | PinManagementMessage::OutputHintChannel { .. }
            | PinManagementMessage::AttachHintSender { .. } => {
                // No-op for output pins / type info / hint channels on a push node
            },
        }
    }

    /// Re-serialize and publish the current catalog state.
    ///
    /// Returns `true` if the catalog was successfully published, `false` on
    /// serialization or write failure (logged at error level).
    fn republish_catalog(
        catalog: &hang::catalog::Catalog,
        catalog_producer: &mut moq_lite::TrackProducer,
    ) -> bool {
        match catalog.to_string() {
            Ok(json) => {
                if let Err(e) = catalog_producer.write_frame(json.into_bytes()) {
                    tracing::error!("Failed to re-publish catalog: {e}");
                    false
                } else {
                    tracing::debug!("MoqPushNode: catalog re-published after dynamic track change");
                    true
                }
            },
            Err(e) => {
                tracing::error!("Failed to serialize updated catalog: {e}");
                false
            },
        }
    }

    /// Create a MoQ track for a newly added dynamic input pin, update the
    /// catalog, and register the receiver for packet forwarding.
    #[allow(clippy::too_many_arguments)]
    fn activate_dynamic_input(
        pin: &InputPin,
        channel: tokio::sync::mpsc::Receiver<Packet>,
        broadcast: &mut moq_lite::BroadcastProducer,
        dynamic_inputs: &mut Vec<DynamicInputState>,
        initial_delay_ms: u64,
        catalog: &mut hang::catalog::Catalog,
        catalog_producer: &mut moq_lite::TrackProducer,
        channels: u32,
        video_codec: VideoCodec,
        audio_codec: AudioCodec,
    ) {
        tracing::info!("MoqPushNode: activated dynamic input pin '{}'", pin.name);

        let is_video = is_video_pin(&pin.name);
        let track_name = track_name_from_pin(&pin.name);

        let track =
            moq_lite::Track { name: track_name.clone(), priority: if is_video { 60 } else { 80 } };
        let producer = match broadcast.create_track(track) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(
                    pin = %pin.name, track = %track_name, error = %e,
                    "MoqPushNode: failed to create MoQ track for dynamic input pin"
                );
                return;
            },
        };

        // Update the catalog with the new track rendition.
        Self::insert_catalog_rendition(
            catalog,
            &track_name,
            is_video,
            channels,
            video_codec,
            audio_codec,
        );

        if !Self::republish_catalog(catalog, catalog_producer) {
            // Roll back — subscribers won't discover this track.
            Self::remove_catalog_rendition(catalog, &track_name);
            // Finish the producer so the broadcast doesn't retain a dangling track.
            let mut tp: hang::container::OrderedProducer = producer.into();
            let _ = tp.track.finish();
            tracing::error!(
                pin = %pin.name, track = %track_name,
                "Skipping dynamic input — catalog republish failed"
            );
            return;
        }

        let clock = MediaClock::new(initial_delay_ms);
        // Guard against duplicate pin names — if a pin with the same name
        // already exists, replace it rather than pushing a second entry
        // (which would leak when RemoveInputPin only removes the first).
        if let Some(pos) = dynamic_inputs.iter().position(|s| s.pin_name == pin.name) {
            let mut old = dynamic_inputs.swap_remove(pos);
            let _ = old.producer.track.finish();
            tracing::warn!(
                pin = %pin.name,
                "Replacing existing dynamic input with same name"
            );
        }
        dynamic_inputs.push(DynamicInputState {
            pin_name: pin.name.clone(),
            receiver: channel,
            producer: producer.into(),
            clock,
            seeded: false,
            first_sent: false,
            is_video,
        });
        tracing::info!(
            pin = %pin.name, track = %track_name,
            "MoqPushNode: dynamic input pin mapped to MoQ track"
        );
    }

    /// Insert a rendition into the catalog for a dynamic track.
    fn insert_catalog_rendition(
        catalog: &mut hang::catalog::Catalog,
        track_name: &str,
        is_video: bool,
        channels: u32,
        video_codec: VideoCodec,
        audio_codec: AudioCodec,
    ) {
        if is_video {
            catalog.video.renditions.insert(
                track_name.to_string(),
                hang::catalog::VideoConfig {
                    codec: catalog_video_codec(video_codec),
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
        } else {
            catalog.audio.renditions.insert(
                track_name.to_string(),
                hang::catalog::AudioConfig {
                    codec: catalog_audio_codec(audio_codec),
                    sample_rate: 48000,
                    channel_count: channels,
                    bitrate: Some(128_000),
                    description: None,
                    container: hang::catalog::Container::default(),
                    jitter: None,
                },
            );
        }
    }

    /// Remove a rendition from the catalog (both audio and video maps).
    fn remove_catalog_rendition(catalog: &mut hang::catalog::Catalog, track_name: &str) {
        catalog.audio.renditions.remove(track_name);
        catalog.video.renditions.remove(track_name);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{is_video_pin, track_name_from_pin};

    /// Regression: `is_video` was previously determined by checking
    /// `pin.accepts_types` for `EncodedVideo`, but since all dynamic pins
    /// accept both audio and video types, it was always `true`. The fix
    /// uses the pin name prefix convention instead.
    #[test]
    fn is_video_from_pin_name_convention() {
        // video/ prefix → video
        assert!(is_video_pin("video/hd"));
        assert!(is_video_pin("video/data"));

        // audio/ prefix → not video
        assert!(!is_video_pin("audio/data"));
        assert!(!is_video_pin("audio/extra"));

        // bare name → not video (defaults to audio)
        assert!(!is_video_pin("in_2"));
        assert!(!is_video_pin("custom_track"));
    }

    /// Verify track name derivation from pin names: pins with an existing
    /// `audio/` or `video/` prefix keep their name; bare names get `audio/`
    /// prepended.
    #[test]
    fn track_name_from_pin_name() {
        assert_eq!(track_name_from_pin("video/hd"), "video/hd");
        assert_eq!(track_name_from_pin("audio/data"), "audio/data");
        assert_eq!(track_name_from_pin("in_2"), "audio/in_2");
        assert_eq!(track_name_from_pin("extra"), "audio/extra");
    }

    /// Verify that `video_codec` config is correctly deserialized from JSON
    /// and that the default (None) falls through to VP9.
    #[test]
    fn video_codec_config_deserialization() {
        use super::MoqPushConfig;

        // Default: video_codec is None
        let default: MoqPushConfig = serde_json::from_str("{}").unwrap();
        assert!(default.video_codec.is_none());

        // Explicit av1
        let av1: MoqPushConfig = serde_json::from_str(r#"{"video_codec": "av1"}"#).unwrap();
        assert_eq!(av1.video_codec.as_deref(), Some("av1"));

        // Explicit vp9
        let vp9: MoqPushConfig = serde_json::from_str(r#"{"video_codec": "vp9"}"#).unwrap();
        assert_eq!(vp9.video_codec.as_deref(), Some("vp9"));
    }
}
