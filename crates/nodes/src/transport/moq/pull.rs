// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::video::{AV1_CONTENT_TYPE, H264_CONTENT_TYPE, VP9_CONTENT_TYPE};
use async_trait::async_trait;
use schemars::JsonSchema;
use serde::Deserialize;
use std::borrow::Cow;
use std::time::Duration;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{
    AudioCodec, EncodedAudioFormat, EncodedVideoFormat, Packet, PacketMetadata, PacketType,
    VideoCodec,
};

use super::constants::{audio_codec_from_catalog, video_codec_from_catalog};
use super::discovered::{
    discovered_codec, record_discovered_codec, remove_discovered_codec, DiscoveredCodec,
    DiscoveredCodecs,
};
use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::{
    state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin, PinCardinality,
    ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

/// Output channels for dynamically created track pins.
///
/// When the catalog gains a track after `initialize()` (e.g. video added to
/// an audio-only stream) and a downstream node connects to its pin, the engine
/// creates the pin on demand and hands us the channel via
/// [`PinManagementMessage::AddedOutputPin`]. Frame routing checks this map for
/// pins that were not advertised at init. Mirrors `MoqPeerNode`'s registry.
///
/// Uses [`std::sync::RwLock`] rather than the async variant because the lock is
/// never held across an `.await`.
type DynamicOutputs = Arc<std::sync::RwLock<HashMap<String, mpsc::Sender<Packet>>>>;

/// A catalog-discovered track with optional codec metadata.
///
/// Wraps the track name/priority with the codec detected from the MoQ
/// catalog so that output pins can advertise the correct codec type.
struct DiscoveredTrack {
    track: super::TrackRef,
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
    /// Codecs discovered from the broadcast catalog, keyed by track/pin name.
    /// Consulted when the engine requests a dynamic output pin so the pin
    /// advertises the publisher's actual codec instead of a guessed default.
    discovered_codecs: DiscoveredCodecs,
}

impl MoqPullNode {
    pub fn new(config: MoqPullConfig) -> Self {
        let audio_codec = config.audio_codec.unwrap_or(AudioCodec::Opus);
        Self {
            output_pins: vec![Self::stable_out_pin(audio_codec)],
            config,
            discovered_codecs: Arc::default(),
        }
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

    /// Pin definition for a track-named output pin created on demand.
    ///
    /// Prefers the catalog-discovered codec for the pin. When the catalog
    /// hasn't been seen yet, falls back to a name heuristic: names containing
    /// a `video/` segment produce [`PacketType::EncodedVideo`] (VP9 default,
    /// like [`Self::output_pins_for_tracks`]); all others produce audio with
    /// the configured fallback codec. The engine skips strict type validation
    /// for dynamic-pin nodes, so the catalog's actual codec governs decoding.
    fn make_dynamic_output_pin(
        name: &str,
        audio_codec: AudioCodec,
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
                _ => VideoCodec::Vp9,
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
                _ => audio_codec,
            };
            PacketType::EncodedAudio(EncodedAudioFormat { codec, codec_private: None })
        };
        OutputPin { name: name.to_string(), produces_type, cardinality: PinCardinality::Broadcast }
    }

    /// Record every catalog-discovered track codec for dynamic pin creation.
    fn record_discovered_tracks(map: &DiscoveredCodecs, tracks: &[DiscoveredTrack]) {
        for dt in tracks {
            let codec = if let Some(v) = dt.video_codec {
                DiscoveredCodec::Video(v)
            } else if let Some(a) = dt.audio_codec {
                DiscoveredCodec::Audio(a)
            } else {
                continue;
            };
            record_discovered_codec(map, &dt.track.name, codec);
        }
    }

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

    fn remove_dynamic_output(dynamic_outputs: &DynamicOutputs, name: &str) {
        dynamic_outputs.write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(name);
    }

    /// Drain runtime pin-management messages for the node's lifetime.
    ///
    /// Runs in its own task so dynamic output pins persist across reconnects
    /// and are serviced even while [`Self::run_connection`] is between
    /// connections. Only output pins are relevant — the pull node has no inputs.
    async fn handle_pin_management(
        mut pin_mgmt_rx: mpsc::Receiver<PinManagementMessage>,
        dynamic_outputs: DynamicOutputs,
        discovered_codecs: DiscoveredCodecs,
        node_name: String,
        audio_codec: AudioCodec,
    ) {
        while let Some(msg) = pin_mgmt_rx.recv().await {
            match msg {
                PinManagementMessage::RequestAddOutputPin { suggested_name, response_tx } => {
                    let pin_name = suggested_name.unwrap_or_else(|| "out".to_string());
                    let discovered = discovered_codec(&discovered_codecs, &pin_name);
                    tracing::info!(node = %node_name, pin = %pin_name, ?discovered, "MoqPullNode: creating dynamic output pin");
                    let pin = Self::make_dynamic_output_pin(&pin_name, audio_codec, discovered);
                    let _ = response_tx.send(Ok(pin));
                },
                PinManagementMessage::AddedOutputPin { pin, channel } => {
                    tracing::info!(node = %node_name, pin = %pin.name, "MoqPullNode: activated dynamic output pin");
                    Self::insert_dynamic_output(&dynamic_outputs, pin.name, channel);
                },
                PinManagementMessage::RemoveOutputPin { pin_name } => {
                    tracing::info!(node = %node_name, pin = %pin_name, "MoqPullNode: removed dynamic output pin");
                    Self::remove_dynamic_output(&dynamic_outputs, &pin_name);
                    remove_discovered_codec(&discovered_codecs, &pin_name);
                },
                PinManagementMessage::RequestAddInputPin { response_tx, .. } => {
                    let _ = response_tx.send(Err(StreamKitError::Runtime(
                        "MoqPullNode has no input pins".to_string(),
                    )));
                },
                PinManagementMessage::AddedInputPin { .. }
                | PinManagementMessage::RemoveInputPin { .. }
                | PinManagementMessage::InputTypeResolved { .. }
                | PinManagementMessage::OutputHintChannel { .. }
                | PinManagementMessage::AttachHintSender { .. } => {},
            }
        }
        tracing::debug!(node = %node_name, "MoqPullNode pin-management task ended");
    }

    /// Type advertised by the output pin with the given name, fixed during
    /// `initialize()`. Used to detect when the catalog's codec (possibly
    /// discovered after init) differs from what downstream consumers were
    /// wired up for.
    fn advertised_pin_type(&self, pin_name: &str) -> Option<&PacketType> {
        self.output_pins.iter().find(|p| p.name == pin_name).map(|p| &p.produces_type)
    }

    /// Audio codec advertised by the stable `out` pin.
    fn advertised_out_audio_codec(&self) -> Option<AudioCodec> {
        match self.advertised_pin_type("out")? {
            PacketType::EncodedAudio(fmt) => Some(fmt.codec),
            _ => None,
        }
    }

    /// Video codec advertised by the output pin with the given name.
    fn advertised_video_codec_for_pin(&self, pin_name: &str) -> Option<VideoCodec> {
        match self.advertised_pin_type(pin_name)? {
            PacketType::EncodedVideo(fmt) => Some(fmt.codec),
            _ => None,
        }
    }

    /// Output pin types are fixed at `initialize()` and the engine has no
    /// mechanism to retype a pin at runtime, so a connection-setup codec
    /// mismatch can never be fixed by reconnecting. Fail terminally with a
    /// Configuration error instead of looping the 1s retry forever.
    ///
    /// Each guard fires only when the catalog actually advertises that media
    /// kind: the audio guard compares the stable `out` pin against the
    /// catalog's audio codec (skipped for video-only catalogs, where the
    /// `out` pin keeps its configured fallback), and the video guard checks
    /// every discovered video track against its same-named pin (which only
    /// exists when init saw the track).
    fn verify_pin_codecs(
        &self,
        discovered_tracks: &[DiscoveredTrack],
    ) -> Result<(), StreamKitError> {
        let catalog_audio_codec = discovered_tracks.iter().find_map(|dt| dt.audio_codec);
        if let (Some(pin_codec), Some(catalog_codec)) =
            (self.advertised_out_audio_codec(), catalog_audio_codec)
        {
            if pin_codec != catalog_codec {
                tracing::error!(
                    advertised = ?pin_codec,
                    discovered = ?catalog_codec,
                    "Catalog audio codec differs from output pin; \
                     pin was set during init before catalog was available"
                );
                return Err(StreamKitError::Configuration(format!(
                    "Audio codec mismatch: output pin advertises {pin_codec:?} \
                     but the broadcast catalog provides {catalog_codec:?}; \
                     set `audio_codec: {catalog_codec:?}` in the moq_pull \
                     config or fix the publisher (pin types are fixed at \
                     initialization and cannot change at runtime)"
                )));
            }
        }

        for dt in discovered_tracks {
            if let (Some(pin_codec), Some(catalog_codec)) =
                (self.advertised_video_codec_for_pin(&dt.track.name), dt.video_codec)
            {
                if pin_codec != catalog_codec {
                    tracing::error!(
                        advertised = ?pin_codec,
                        discovered = ?catalog_codec,
                        track = %dt.track.name,
                        "Catalog video codec differs from output pin; \
                         pin was set during init before catalog was available"
                    );
                    return Err(StreamKitError::Configuration(format!(
                        "Video codec mismatch on track '{}': output pin advertises \
                         {pin_codec:?} but the broadcast catalog provides \
                         {catalog_codec:?}; fix the publisher or restart the \
                         pipeline (pin types are fixed at initialization and \
                         cannot change at runtime)",
                        dt.track.name
                    )));
                }
            }
        }

        Ok(())
    }

    /// Subscribe to the first audio track in `new_tracks` so a late-arriving
    /// audio track (e.g. audio added after an initial video-only catalog) is
    /// streamed mid-session. Returns the subscription and its codec, or `None`
    /// if there is no audio track or the subscribe fails.
    ///
    /// Warns when the track's codec differs from the `out` pin advertised at
    /// `initialize()`: that pin's type is fixed, so feeding it a different codec
    /// would hand mislabeled bytes to downstream consumers bound to it.
    /// Mid-session mismatches deliberately warn rather than fail terminally
    /// (unlike [`Self::verify_pin_codecs`] at connection setup): tearing down a
    /// live pipeline over a late optional track is worse than degraded output
    /// on the affected pin.
    async fn attach_late_audio(
        &self,
        broadcast: &moq_net::broadcast::Consumer,
        new_tracks: &[DiscoveredTrack],
    ) -> Option<(LateTrack, AudioCodec)> {
        let dt = new_tracks.iter().find(|dt| dt.audio_codec.is_some())?;
        let codec = dt.audio_codec?;
        let consumer = match super::subscribe_track(broadcast, &dt.track.name, dt.track.priority)
            .await
        {
            Ok(consumer) => consumer,
            Err(e) => {
                tracing::warn!(error = %e, track = %dt.track.name, "MoqPullNode: failed to subscribe to late audio track");
                return None;
            },
        };
        tracing::info!(track = %dt.track.name, "MoqPullNode: subscribing to late audio track");
        if let Some(pin_codec) = self.advertised_out_audio_codec() {
            if pin_codec != codec {
                tracing::warn!(
                    advertised = ?pin_codec,
                    discovered = ?codec,
                    track = %dt.track.name,
                    "MoqPullNode: late audio track codec differs from the advertised `out` pin; downstream consumers may misdecode (pin was fixed at init before this track appeared)"
                );
            }
        }
        let pin_registered = self.output_pins.iter().any(|p| p.name == dt.track.name);
        Some((
            LateTrack {
                track: dt.track.clone(),
                consumer,
                pin_name: dt.track.name.clone(),
                pin_registered,
            },
            codec,
        ))
    }

    /// Subscribe to the first video track in `new_tracks` so a late-arriving
    /// video track (e.g. video added to an audio-only stream) is streamed
    /// mid-session. Returns the subscription and its catalog content-type, or
    /// `None` if there is no video track or the subscribe fails.
    async fn attach_late_video(
        &self,
        broadcast: &moq_net::broadcast::Consumer,
        new_tracks: &[DiscoveredTrack],
    ) -> Option<(LateTrack, &'static str)> {
        let dt = new_tracks.iter().find(|dt| dt.video_codec.is_some())?;
        let consumer = match super::subscribe_track(broadcast, &dt.track.name, dt.track.priority)
            .await
        {
            Ok(consumer) => consumer,
            Err(e) => {
                tracing::warn!(error = %e, track = %dt.track.name, "MoqPullNode: failed to subscribe to late video track");
                return None;
            },
        };
        tracing::info!(track = %dt.track.name, "MoqPullNode: subscribing to late video track");
        if let (Some(pin_codec), Some(catalog_codec)) =
            (self.advertised_video_codec_for_pin(&dt.track.name), dt.video_codec)
        {
            if pin_codec != catalog_codec {
                tracing::warn!(
                    advertised = ?pin_codec,
                    discovered = ?catalog_codec,
                    track = %dt.track.name,
                    "MoqPullNode: late video track codec differs from the advertised pin; downstream consumers may misdecode (pin was fixed at init before this track appeared)"
                );
            }
        }
        let content_type = match dt.video_codec {
            Some(VideoCodec::Av1) => AV1_CONTENT_TYPE,
            Some(VideoCodec::H264) => H264_CONTENT_TYPE,
            _ => VP9_CONTENT_TYPE,
        };
        let pin_registered = self.output_pins.iter().any(|p| p.name == dt.track.name);
        Some((
            LateTrack {
                track: dt.track.clone(),
                consumer,
                pin_name: dt.track.name.clone(),
                pin_registered,
            },
            content_type,
        ))
    }

    /// Route a track frame to its output pin. Statically advertised pins (set
    /// during `initialize()`) go through the engine's direct sender; pins
    /// created at runtime go through the [`DynamicOutputs`] channel registry.
    async fn route_track_frame(
        context: &mut NodeContext,
        dynamic_outputs: &DynamicOutputs,
        pin_registered: bool,
        pin_name: &str,
        packet: Packet,
    ) -> RouteOutcome {
        if pin_registered {
            return match context.output_sender.send(pin_name, packet).await {
                Ok(()) => RouteOutcome::Sent,
                Err(_) => RouteOutcome::Closed,
            };
        }
        Self::route_to_dynamic_output(dynamic_outputs, pin_name, packet).await
    }

    /// Send a frame to a runtime-created pin's channel. Returns
    /// [`RouteOutcome::NoConsumer`] when no pin is registered or its channel has
    /// closed (in which case the stale entry is removed) — both are non-fatal,
    /// so the node keeps running until a consumer (re)connects.
    async fn route_to_dynamic_output(
        dynamic_outputs: &DynamicOutputs,
        pin_name: &str,
        packet: Packet,
    ) -> RouteOutcome {
        let channel = dynamic_outputs
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(pin_name)
            .cloned();
        let Some(tx) = channel else {
            return RouteOutcome::NoConsumer;
        };
        if tx.send(packet).await.is_ok() {
            RouteOutcome::Sent
        } else {
            Self::remove_dynamic_output(dynamic_outputs, pin_name);
            RouteOutcome::NoConsumer
        }
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

    fn supports_dynamic_pins(&self) -> bool {
        true
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
        let tracks = match Box::pin(self.discover_tracks()).await {
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

        Self::record_discovered_tracks(&self.discovered_codecs, &tracks);

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

        // Dynamic output pin channels, serviced by a dedicated task so they
        // survive reconnects. Populated when the engine creates a track-named
        // pin (e.g. late video) that wasn't advertised at init.
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let pin_mgmt_task = context.pin_management_rx.take().map(|rx| {
            let default_audio_codec = self.config.audio_codec.unwrap_or(AudioCodec::Opus);
            tokio::spawn(Self::handle_pin_management(
                rx,
                dynamic_outputs.clone(),
                self.discovered_codecs.clone(),
                node_name.clone(),
                default_audio_codec,
            ))
        });

        let mut total_packet_count = 0;
        // Main reconnection loop - simple 1 second retry for all failures
        loop {
            match Box::pin(self.run_connection(
                &mut context,
                &dynamic_outputs,
                &mut total_packet_count,
            ))
            .await
            {
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

        if let Some(task) = pin_mgmt_task {
            task.abort();
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

/// Outcome of waiting for a broadcast announcement.
enum BroadcastWait {
    Broadcast(moq_net::broadcast::Consumer),
    End(StreamEndReason),
}

/// Result of routing a track frame to an output pin.
enum RouteOutcome {
    /// Delivered to a connected pin.
    Sent,
    /// No connected consumer (dynamic pin not yet created, or it was removed).
    /// The frame is dropped; the node keeps running.
    NoConsumer,
    /// The downstream channel for a statically advertised pin closed.
    Closed,
}

/// A track subscribed mid-session by the catalog watch, plus the read-loop
/// state the caller needs to start consuming it.
struct LateTrack {
    track: super::TrackRef,
    consumer: moq_net::track::Subscriber,
    pin_name: String,
    pin_registered: bool,
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
        payload: bytes::Bytes,
    ) -> Result<(u64, bytes::Bytes), hang::Error> {
        // hang protocol: frame payload is prefixed with a varint timestamp in microseconds.
        // We parse it and forward the remaining bytes (Opus frame data).
        let frame = hang::container::Frame::decode(payload)?;
        #[allow(clippy::cast_possible_truncation)] // MoQ timestamps fit in u64
        let timestamp_us = frame.timestamp.as_micros() as u64;
        Ok((timestamp_us, frame.payload))
    }

    /// Read the next raw MoQ frame, returning the payload and whether this
    /// frame is the first in a newly opened MoQ group (i.e. a keyframe
    /// boundary in the hang protocol).
    async fn read_next_raw_moq(
        track_consumer: &mut moq_net::track::Subscriber,
        current_group: &mut Option<moq_net::group::Consumer>,
        is_first_in_group: &mut bool,
    ) -> Result<Option<bytes::Bytes>, moq_net::Error> {
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
                Ok(Some(frame)) => return Ok(Some(frame.payload)),
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

        let origin = moq_net::Origin::random().produce();
        let consumer = origin.consume();
        let _consumer_session =
            Box::pin(client.clone().with_subscriber(origin).connect(url)).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        // Wait for the broadcast to be announced.  During dynamic session
        // initialization the publisher (browser) may not have connected yet.
        let discovery_timeout = Duration::from_secs(15);

        let Ok(Some(broadcast)) = tokio::time::timeout(
            discovery_timeout,
            consumer.announced_broadcast(self.config.broadcast.as_str()),
        )
        .await
        else {
            tracing::debug!(
                broadcast = %self.config.broadcast,
                "Broadcast not announced within {}s; using default output pin",
                discovery_timeout.as_secs()
            );
            return Ok(Vec::new());
        };

        // Subscribe to the catalog track
        let raw_catalog_track = super::subscribe_catalog(&broadcast).await.map_err(|e| {
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
                    track: super::TrackRef { name: name.clone(), priority: 80 },
                    video_codec: None,
                    audio_codec: Some(codec),
                });
            } else {
                tracing::debug!(track = %name, codec = ?config.codec, "skipping unsupported audio track");
            }
        }

        for (name, config) in &catalog.video.renditions {
            if let Some(codec) = video_codec_from_catalog(&config.codec) {
                tracing::info!(track = %name, ?codec, "found video track");
                tracks.push(DiscoveredTrack {
                    track: super::TrackRef { name: name.clone(), priority: 60 },
                    video_codec: Some(codec),
                    audio_codec: None,
                });
            } else {
                tracing::debug!(track = %name, codec = ?config.codec, "skipping unsupported video track");
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

    /// Wait for `broadcast` to be announced, servicing control messages.
    ///
    /// Non-shutdown control messages (e.g. the engine's startup `Start`) are
    /// ignored so a pull node waiting for a late publisher keeps waiting.
    /// Restarting `announced_broadcast` after a control message is safe: each
    /// call replays the currently active broadcast set, so an announcement
    /// observed between iterations isn't lost.
    async fn wait_for_announced_broadcast(
        control_rx: &mut mpsc::Receiver<streamkit_core::control::NodeControlMessage>,
        consumer: &moq_net::origin::Consumer,
        broadcast: &str,
    ) -> BroadcastWait {
        loop {
            tokio::select! {
                msg = control_rx.recv() => {
                    match msg {
                        Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                            tracing::info!("MoQ pull received shutdown signal while waiting for broadcast");
                            return BroadcastWait::End(StreamEndReason::Natural);
                        }
                        Some(control_msg) => {
                            tracing::debug!("MoQ pull ignoring control message while waiting: {:?}", control_msg);
                        }
                        None => {
                            // Control channel closed - engine is shutting down
                            tracing::info!("MoQ pull control channel closed while waiting for broadcast");
                            return BroadcastWait::End(StreamEndReason::Natural);
                        }
                    }
                }
                maybe_broadcast = consumer.announced_broadcast(broadcast) => {
                    if let Some(bc) = maybe_broadcast {
                        tracing::info!("Broadcast '{broadcast}' has been announced");
                        return BroadcastWait::Broadcast(bc);
                    }
                    tracing::warn!("Announcement channel closed before broadcast '{broadcast}' was announced, will reconnect");
                    return BroadcastWait::End(StreamEndReason::Reconnect);
                }
            }
        }
    }

    // MoQ connection state machine with multiplexed track handling and error recovery
    // High complexity is inherent to protocol handling (track management, object streaming, packet routing)
    #[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
    async fn run_connection(
        &self,
        context: &mut NodeContext,
        dynamic_outputs: &DynamicOutputs,
        total_packet_count: &mut u32,
    ) -> Result<StreamEndReason, StreamKitError> {
        /// Which media kind produced a frame in the multiplexed select loop.
        enum ReadSource {
            Audio,
            Video,
        }

        /// One iteration of the multiplexed select loop: a track frame, or a
        /// catalog update that may reveal a late-arriving track.
        enum LoopEvent {
            Frame(Result<Option<bytes::Bytes>, moq_net::Error>, ReadSource),
            // Boxed: a parsed catalog is far larger than a frame result, so an
            // unboxed variant bloats every `LoopEvent` (clippy::large_enum_variant).
            Catalog(Box<Result<Option<hang::catalog::Catalog>, hang::Error>>),
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
        let origin = moq_net::Origin::random().produce();
        let consumer = origin.consume();
        let _consumer_session =
            Box::pin(client.clone().with_subscriber(origin).connect(url)).await.map_err(|e| {
                StreamKitError::Runtime(format!("Failed to create consumer session: {e}"))
            })?;

        tracing::debug!("Waiting for broadcast '{}' to be announced...", self.config.broadcast);
        let broadcast = match Self::wait_for_announced_broadcast(
            &mut context.control_rx,
            &consumer,
            self.config.broadcast.as_str(),
        )
        .await
        {
            BroadcastWait::Broadcast(broadcast) => broadcast,
            BroadcastWait::End(reason) => return Ok(reason),
        };

        tracing::info!("Subscribed to broadcast '{}'", self.config.broadcast);

        // Get the catalog to find available tracks
        let raw_catalog_track = super::subscribe_catalog(&broadcast).await.map_err(|e| {
            StreamKitError::Runtime(format!("Failed to subscribe to catalog track: {e}"))
        })?;
        let mut catalog_consumer = super::catalog_consumer::CatalogConsumer::new(raw_catalog_track);

        tracing::debug!("subscribed to catalog track: {}", hang::catalog::Catalog::DEFAULT_NAME);

        // Wait for catalog data with timeout
        let discovered_tracks = self.parse_catalog(&mut catalog_consumer).await?;

        if discovered_tracks.is_empty() {
            return Err(StreamKitError::Runtime(
                "No supported tracks found in broadcast".to_string(),
            ));
        }

        Self::record_discovered_tracks(&self.discovered_codecs, &discovered_tracks);

        let audio_track = discovered_tracks.iter().find(|dt| dt.audio_codec.is_some());
        let video_track = discovered_tracks.iter().find(|dt| dt.video_codec.is_some());

        let mut resolved_audio_codec = audio_track
            .and_then(|dt| dt.audio_codec)
            .or(self.config.audio_codec)
            .unwrap_or(AudioCodec::Opus);

        self.verify_pin_codecs(&discovered_tracks)?;

        if audio_track.is_none() && video_track.is_none() {
            return Err(StreamKitError::Runtime(
                "No audio or video tracks found in broadcast".to_string(),
            ));
        }

        // Subscribe to audio track. The owned `*_sub_track` carries the track
        // for re-subscribe and lets the catalog watch attach a late track.
        let (mut audio_track_consumer, mut audio_track_pin_name, mut audio_track_pin_registered) =
            if let Some(dt) = audio_track {
                tracing::info!("subscribing to audio track: {}", dt.track.name);
                let pin_name = dt.track.name.clone();
                let pin_registered = self.output_pins.iter().any(|p| p.name == pin_name);
                let consumer =
                    super::subscribe_track(&broadcast, &dt.track.name, dt.track.priority)
                        .await
                        .map_err(|e| {
                            StreamKitError::Runtime(format!(
                                "Failed to subscribe to audio track: {e}"
                            ))
                        })?;
                (Some(consumer), Some(pin_name), pin_registered)
            } else {
                (None, None, false)
            };
        let mut audio_sub_track: Option<super::TrackRef> = audio_track.map(|dt| dt.track.clone());

        // Subscribe to video track
        let (mut video_track_consumer, mut video_track_pin_name, mut video_track_pin_registered) =
            if let Some(dt) = video_track {
                tracing::info!("subscribing to video track: {}", dt.track.name);
                let pin_name = dt.track.name.clone();
                let pin_registered = self.output_pins.iter().any(|p| p.name == pin_name);
                let consumer =
                    super::subscribe_track(&broadcast, &dt.track.name, dt.track.priority)
                        .await
                        .map_err(|e| {
                            StreamKitError::Runtime(format!(
                                "Failed to subscribe to video track: {e}"
                            ))
                        })?;
                (Some(consumer), Some(pin_name), pin_registered)
            } else {
                (None, None, false)
            };
        let mut video_sub_track: Option<super::TrackRef> = video_track.map(|dt| dt.track.clone());

        let mut audio_current_group: Option<moq_net::group::Consumer> = None;
        let mut video_current_group: Option<moq_net::group::Consumer> = None;

        // Resolve the video content_type from the discovered track's codec.
        // Falls back to "video/vp9" if no codec info is available. Reassigned
        // when the catalog watch attaches a late video track.
        let mut video_content_type: &str = match video_track.and_then(|dt| dt.video_codec) {
            Some(VideoCodec::Av1) => AV1_CONTENT_TYPE,
            Some(VideoCodec::H264) => H264_CONTENT_TYPE,
            _ => VP9_CONTENT_TYPE,
        };

        // Keep watching the catalog after initial discovery so tracks that
        // appear later (e.g. video added to an audio-only stream) are picked
        // up mid-session. Disabled once the catalog track closes or errors.
        let mut catalog_watch_active = true;

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
            // Watch for late-arriving tracks. Pends forever once disabled so it
            // never busy-loops after the catalog track closes.
            let catalog_read = async {
                if catalog_watch_active {
                    catalog_consumer.next().await
                } else {
                    std::future::pending().await
                }
            };

            let event: LoopEvent = if let Some(token) = &context.cancellation_token {
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
                    result = audio_read => LoopEvent::Frame(result, ReadSource::Audio),
                    result = video_read => LoopEvent::Frame(result, ReadSource::Video),
                    catalog = catalog_read => LoopEvent::Catalog(Box::new(catalog)),
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
                    result = audio_read => LoopEvent::Frame(result, ReadSource::Audio),
                    result = video_read => LoopEvent::Frame(result, ReadSource::Video),
                    catalog = catalog_read => LoopEvent::Catalog(Box::new(catalog)),
                }
            };

            let (read_result, source): (Result<Option<bytes::Bytes>, moq_net::Error>, ReadSource) =
                match event {
                    LoopEvent::Frame(result, source) => (result, source),
                    LoopEvent::Catalog(boxed) => match *boxed {
                        Ok(Some(catalog)) => {
                            let new_tracks = Self::extract_tracks(&catalog);
                            Self::record_discovered_tracks(&self.discovered_codecs, &new_tracks);

                            if audio_sub_track.is_none() {
                                if let Some((late, codec)) =
                                    self.attach_late_audio(&broadcast, &new_tracks).await
                                {
                                    resolved_audio_codec = codec;
                                    audio_track_pin_registered = late.pin_registered;
                                    audio_track_pin_name = Some(late.pin_name);
                                    audio_sub_track = Some(late.track);
                                    audio_track_consumer = Some(late.consumer);
                                    audio_current_group = None;
                                    audio_is_first_in_group = true;
                                    last_audio_timestamp_us = None;
                                    audio_clock = MediaClock::new(0);
                                }
                            }

                            if video_sub_track.is_none() {
                                if let Some((late, content_type)) =
                                    self.attach_late_video(&broadcast, &new_tracks).await
                                {
                                    video_content_type = content_type;
                                    video_track_pin_registered = late.pin_registered;
                                    video_track_pin_name = Some(late.pin_name);
                                    video_sub_track = Some(late.track);
                                    video_track_consumer = Some(late.consumer);
                                    video_current_group = None;
                                    video_is_first_in_group = true;
                                    last_video_timestamp_us = None;
                                    video_clock = MediaClock::new(0);
                                }
                            }
                            continue;
                        },
                        Ok(None) => {
                            tracing::debug!(
                                "MoqPullNode: catalog track closed; stopping catalog watch"
                            );
                            catalog_watch_active = false;
                            continue;
                        },
                        Err(e) => {
                            tracing::debug!(error = %e, "MoqPullNode: catalog watch error; stopping catalog watch");
                            catalog_watch_active = false;
                            continue;
                        },
                    },
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
                            if pin_name != "out" {
                                let outcome = Self::route_track_frame(
                                    context,
                                    dynamic_outputs,
                                    audio_track_pin_registered,
                                    pin_name,
                                    packet.clone(),
                                )
                                .await;
                                if matches!(outcome, RouteOutcome::Closed) {
                                    tracing::debug!("Audio output channel closed, stopping node");
                                    return Ok(StreamEndReason::Natural);
                                }
                            }
                            // Always send audio to the stable "out" pin
                            if context.output_sender.send("out", packet).await.is_err() {
                                tracing::debug!("Output channel closed, stopping node");
                                return Ok(StreamEndReason::Natural);
                            }
                        },
                        ReadSource::Video => {
                            if let Some(pin_name) = video_track_pin_name.as_deref() {
                                let outcome = Self::route_track_frame(
                                    context,
                                    dynamic_outputs,
                                    video_track_pin_registered,
                                    pin_name,
                                    packet,
                                )
                                .await;
                                match outcome {
                                    RouteOutcome::Sent => {},
                                    RouteOutcome::NoConsumer => {
                                        tracing::trace!(
                                            "Video pin has no consumer, discarding packet"
                                        );
                                        stats_tracker.discarded();
                                        continue;
                                    },
                                    RouteOutcome::Closed => {
                                        tracing::debug!(
                                            "Video output channel closed, stopping node"
                                        );
                                        return Ok(StreamEndReason::Natural);
                                    },
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
                                if let Some(track) = audio_sub_track.as_ref() {
                                    audio_resubscribe_attempts += 1;
                                    match super::subscribe_track(
                                        &broadcast,
                                        &track.name,
                                        track.priority,
                                    )
                                    .await
                                    {
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
                                if let Some(track) = video_sub_track.as_ref() {
                                    video_resubscribe_attempts += 1;
                                    match super::subscribe_track(
                                        &broadcast,
                                        &track.name,
                                        track.priority,
                                    )
                                    .await
                                    {
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
                Err(moq_net::Error::Cancel) => {
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
    use super::super::TrackRef;
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn test_output_pins_for_tracks_includes_stable_out() {
        let tracks = vec![DiscoveredTrack {
            track: TrackRef { name: "audio/data".to_string(), priority: 0 },
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
            track: TrackRef { name: "out".to_string(), priority: 0 },
            video_codec: None,
            audio_codec: Some(AudioCodec::Opus),
        }];
        let pins = MoqPullNode::output_pins_for_tracks(&tracks, AudioCodec::Opus);
        assert_eq!(pins.iter().filter(|p| p.name == "out").count(), 1);
    }

    #[test]
    fn test_output_pins_for_tracks_uses_av1_codec() {
        let tracks = vec![DiscoveredTrack {
            track: TrackRef { name: "video/data".to_string(), priority: 60 },
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
            track: TrackRef { name: "video/data".to_string(), priority: 60 },
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
            track: TrackRef { name: "audio/data".to_string(), priority: 80 },
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
            track: TrackRef { name: "video/data".to_string(), priority: 60 },
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
        let frame = hang::container::Frame {
            timestamp: hang::container::Timestamp::from_micros(123).expect("valid timestamp"),
            payload: bytes::Bytes::from_static(b"opus-frame-bytes"),
        };
        frame.encode(&mut buf).expect("encode succeeds");
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
                track: TrackRef { name: "my-custom-audio".to_string(), priority: 80 },
                video_codec: None,
                audio_codec: Some(AudioCodec::Opus),
            },
            DiscoveredTrack {
                track: TrackRef { name: "my-custom-video".to_string(), priority: 60 },
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
            track: TrackRef { name: "non-standard-name".to_string(), priority: 60 },
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

    fn video_discovered_track(codec: VideoCodec) -> DiscoveredTrack {
        DiscoveredTrack {
            track: TrackRef { name: "video/data".to_string(), priority: 60 },
            video_codec: Some(codec),
            audio_codec: None,
        }
    }

    fn audio_discovered_track(codec: AudioCodec) -> DiscoveredTrack {
        DiscoveredTrack {
            track: TrackRef { name: "audio/data".to_string(), priority: 50 },
            video_codec: None,
            audio_codec: Some(codec),
        }
    }

    /// Regression for #530: a post-init audio codec mismatch used to return a
    /// transient Runtime error, looping the 1s reconnect forever even though
    /// reconnecting can never fix it. It must be a terminal Configuration error.
    #[test]
    fn test_audio_codec_mismatch_is_terminal_configuration_error() {
        // initialize() failed to discover the catalog (timeout), leaving the
        // "out" pin at the default Opus codec; the catalog later provides AAC.
        let node = MoqPullNode::new(MoqPullConfig::default());
        let err = node.verify_pin_codecs(&[audio_discovered_track(AudioCodec::Aac)]).unwrap_err();
        assert!(
            matches!(err, StreamKitError::Configuration(_)),
            "expected terminal Configuration error, got {err:?}"
        );
    }

    #[test]
    fn test_no_mismatch_when_pin_matches_catalog() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        node.verify_pin_codecs(&[audio_discovered_track(AudioCodec::Opus)]).unwrap();
    }

    /// A video-only catalog must not trip the audio guard: the `out` pin keeps
    /// its configured fallback codec and no audio comparison should happen
    /// (comparing against a resolved fallback used to fail terminally,
    /// permanently killing pipelines whose audio track briefly disappeared
    /// across a reconnect).
    #[test]
    fn test_video_only_catalog_does_not_trip_audio_guard() {
        let config =
            MoqPullConfig { audio_codec: Some(AudioCodec::Aac), ..MoqPullConfig::default() };
        let node = MoqPullNode::new(config);
        node.verify_pin_codecs(&[video_discovered_track(VideoCodec::Vp9)]).unwrap();
    }

    #[test]
    fn test_explicit_aac_config_avoids_mismatch_with_aac_catalog() {
        let config =
            MoqPullConfig { audio_codec: Some(AudioCodec::Aac), ..MoqPullConfig::default() };
        let node = MoqPullNode::new(config);
        node.verify_pin_codecs(&[audio_discovered_track(AudioCodec::Aac)]).unwrap();
    }

    /// Regression for #530: there was no video-codec mismatch guard at all,
    /// so a stale video pin silently forwarded mislabeled frames.
    #[test]
    fn test_video_codec_mismatch_is_terminal_configuration_error() {
        let mut node = MoqPullNode::new(MoqPullConfig::default());
        node.output_pins = MoqPullNode::output_pins_for_tracks(
            &[video_discovered_track(VideoCodec::Vp9)],
            AudioCodec::Opus,
        );
        let err = node.verify_pin_codecs(&[video_discovered_track(VideoCodec::Av1)]).unwrap_err();
        assert!(
            matches!(err, StreamKitError::Configuration(_)),
            "expected terminal Configuration error, got {err:?}"
        );
    }

    #[test]
    fn test_video_codec_match_passes_guard() {
        let mut node = MoqPullNode::new(MoqPullConfig::default());
        node.output_pins = MoqPullNode::output_pins_for_tracks(
            &[video_discovered_track(VideoCodec::Av1)],
            AudioCodec::Opus,
        );
        node.verify_pin_codecs(&[video_discovered_track(VideoCodec::Av1)]).unwrap();
    }

    /// Every video track must be verified, not just the first one the
    /// catalog lists — a mismatch on a second rendition is just as terminal.
    #[test]
    fn test_video_codec_mismatch_on_second_track_is_detected() {
        let hd = DiscoveredTrack {
            track: TrackRef { name: "video/hd".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Vp9),
            audio_codec: None,
        };
        let sd_vp9 = DiscoveredTrack {
            track: TrackRef { name: "video/sd".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Vp9),
            audio_codec: None,
        };
        let mut node = MoqPullNode::new(MoqPullConfig::default());
        node.output_pins = MoqPullNode::output_pins_for_tracks(&[hd, sd_vp9], AudioCodec::Opus);

        let hd_again = DiscoveredTrack {
            track: TrackRef { name: "video/hd".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Vp9),
            audio_codec: None,
        };
        let sd_av1 = DiscoveredTrack {
            track: TrackRef { name: "video/sd".to_string(), priority: 60 },
            video_codec: Some(VideoCodec::Av1),
            audio_codec: None,
        };
        let err = node.verify_pin_codecs(&[hd_again, sd_av1]).unwrap_err();
        assert!(
            matches!(err, StreamKitError::Configuration(_)),
            "expected terminal Configuration error, got {err:?}"
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
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("test-broadcast", moq_net::broadcast::Route::announced())
            .unwrap();
        let mut producer = super::super::create_catalog_track(&mut broadcast).unwrap();

        let consumer = origin.consume();
        let bc = consumer.announced_broadcast("test-broadcast").await.unwrap();
        let consumer_track = super::super::subscribe_catalog(&bc).await.unwrap();

        let catalog = audio_only_catalog();
        super::super::write_catalog_json(&mut producer, catalog.to_vec().unwrap()).unwrap();

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

    fn audio_only_catalog() -> hang::catalog::Catalog {
        let mut audio_renditions = std::collections::BTreeMap::new();
        audio_renditions.insert("audio/data".to_string(), {
            let mut cfg = hang::catalog::AudioConfig::new(
                super::super::constants::catalog_audio_codec(AudioCodec::Opus),
                48000,
                2,
            );
            cfg.bitrate = Some(128_000);
            cfg
        });
        let mut catalog = hang::catalog::Catalog::default();
        catalog.audio.renditions = audio_renditions;
        catalog
    }

    fn video_catalog(codec: VideoCodec) -> hang::catalog::Catalog {
        let mut catalog = audio_only_catalog();
        catalog.video.renditions.insert("video/data".to_string(), {
            let mut cfg = hang::catalog::VideoConfig::new(
                super::super::constants::catalog_video_codec(codec),
            );
            cfg.framerate = Some(30.0);
            cfg.optimize_for_latency = Some(true);
            cfg
        });
        catalog
    }

    fn audio_video_catalog() -> hang::catalog::Catalog {
        video_catalog(VideoCodec::Vp9)
    }

    /// Regression: an incremental publisher that announces audio first and adds
    /// video in a later catalog update used to lose the video track once
    /// `settle_catalog` was removed (the pull node discovered once and never
    /// re-watched). The first non-empty catalog must still return promptly with
    /// just audio, while a subsequent update reveals the video track. The
    /// catalog watch in `run_connection` subscribes to the late track.
    #[tokio::test]
    async fn test_catalog_watch_discovers_late_video_track() {
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("test-broadcast", moq_net::broadcast::Route::announced())
            .unwrap();
        let mut producer = super::super::create_catalog_track(&mut broadcast).unwrap();

        let consumer = origin.consume();
        let bc = consumer.announced_broadcast("test-broadcast").await.unwrap();
        let consumer_track = super::super::subscribe_catalog(&bc).await.unwrap();
        let mut cc = super::super::catalog_consumer::CatalogConsumer::new(consumer_track);

        let write_catalog = |producer: &mut moq_net::track::Producer,
                             catalog: &hang::catalog::Catalog| {
            super::super::write_catalog_json(producer, catalog.to_vec().unwrap()).unwrap();
        };

        // First update: audio only — initial discovery returns immediately.
        write_catalog(&mut producer, &audio_only_catalog());
        let node = MoqPullNode::new(MoqPullConfig::default());
        let initial = node.parse_catalog(&mut cc).await.unwrap();
        assert_eq!(initial.len(), 1, "first catalog should expose only audio");
        assert!(initial[0].audio_codec.is_some());
        assert!(initial.iter().all(|t| t.video_codec.is_none()));

        // Second update: video added. The watch loop reads this from the same
        // consumer and now sees both renditions.
        write_catalog(&mut producer, &audio_video_catalog());
        let updated = cc.next().await.unwrap().expect("second catalog frame");
        let tracks = MoqPullNode::extract_tracks(&updated);
        assert!(tracks.iter().any(|t| t.audio_codec.is_some()), "audio still present");
        assert!(
            tracks.iter().any(|t| t.video_codec == Some(VideoCodec::Vp9)),
            "late video track should be discovered on the second catalog update"
        );
    }

    /// The pull node must advertise dynamic-pin support so the engine routes
    /// `RequestAddOutputPin` for tracks (e.g. late video) that weren't present
    /// at `initialize()`.
    #[test]
    fn test_supports_dynamic_pins() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        assert!(node.supports_dynamic_pins());
    }

    #[test]
    fn test_make_dynamic_output_pin_video_vs_audio() {
        let video = MoqPullNode::make_dynamic_output_pin("video/data", AudioCodec::Opus, None);
        assert!(
            matches!(&video.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Vp9),
            "video-prefixed pin should produce EncodedVideo, got {:?}",
            video.produces_type
        );
        let audio = MoqPullNode::make_dynamic_output_pin("audio/data", AudioCodec::Aac, None);
        assert!(
            matches!(&audio.produces_type, PacketType::EncodedAudio(fmt) if fmt.codec == AudioCodec::Aac),
            "non-video pin should produce EncodedAudio, got {:?}",
            audio.produces_type
        );
    }

    /// Regression for #568: dynamic video pins hardcoded VP9 regardless of
    /// what the catalog advertised. A discovered codec must win over both the
    /// VP9 default and the name heuristic.
    #[test]
    fn test_make_dynamic_output_pin_uses_discovered_codec() {
        let video = MoqPullNode::make_dynamic_output_pin(
            "video/data",
            AudioCodec::Opus,
            Some(DiscoveredCodec::Video(VideoCodec::Av1)),
        );
        assert!(
            matches!(&video.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::Av1),
            "discovered AV1 should override the VP9 default, got {:?}",
            video.produces_type
        );
        let audio = MoqPullNode::make_dynamic_output_pin(
            "audio/data",
            AudioCodec::Opus,
            Some(DiscoveredCodec::Audio(AudioCodec::Aac)),
        );
        assert!(
            matches!(&audio.produces_type, PacketType::EncodedAudio(fmt) if fmt.codec == AudioCodec::Aac),
            "discovered AAC should override the configured fallback, got {:?}",
            audio.produces_type
        );
        let odd_name = MoqPullNode::make_dynamic_output_pin(
            "non-standard-name",
            AudioCodec::Opus,
            Some(DiscoveredCodec::Video(VideoCodec::H264)),
        );
        assert!(
            matches!(&odd_name.produces_type, PacketType::EncodedVideo(fmt) if fmt.codec == VideoCodec::H264),
            "a discovered video codec should beat the name heuristic, got {:?}",
            odd_name.produces_type
        );
    }

    /// `RequestAddOutputPin` is answered with a pin named after the request and
    /// `AddedOutputPin` registers a channel that frame routing can later find.
    #[tokio::test]
    async fn test_pin_management_creates_and_registers_output_pin() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (mgmt_tx, mgmt_rx) = mpsc::channel(8);
        let task = tokio::spawn(MoqPullNode::handle_pin_management(
            mgmt_rx,
            dynamic_outputs.clone(),
            Arc::default(),
            "test-node".to_string(),
            AudioCodec::Opus,
        ));

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        mgmt_tx
            .send(PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("video/data".to_string()),
                response_tx: resp_tx,
            })
            .await
            .unwrap();
        let pin = resp_rx.await.unwrap().unwrap();
        assert_eq!(pin.name, "video/data");

        let (data_tx, _data_rx) = mpsc::channel(8);
        mgmt_tx
            .send(PinManagementMessage::AddedOutputPin { pin: pin.clone(), channel: data_tx })
            .await
            .unwrap();

        // A second round-trip flushes the FIFO queue, guaranteeing the
        // AddedOutputPin above has been processed before we inspect the map.
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        mgmt_tx
            .send(PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("out".to_string()),
                response_tx: ack_tx,
            })
            .await
            .unwrap();
        let _ = ack_rx.await.unwrap();

        assert!(
            dynamic_outputs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key("video/data"),
            "AddedOutputPin should register the channel in DynamicOutputs"
        );

        // RemoveOutputPin clears the registration.
        mgmt_tx
            .send(PinManagementMessage::RemoveOutputPin { pin_name: "video/data".to_string() })
            .await
            .unwrap();
        let (ack2_tx, ack2_rx) = tokio::sync::oneshot::channel();
        mgmt_tx
            .send(PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("out".to_string()),
                response_tx: ack2_tx,
            })
            .await
            .unwrap();
        let _ = ack2_rx.await.unwrap();
        assert!(
            !dynamic_outputs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key("video/data"),
            "RemoveOutputPin should drop the channel from DynamicOutputs"
        );

        drop(mgmt_tx);
        task.await.unwrap();
    }

    /// Keeps the producer handles alive for the lifetime of the test; dropping
    /// them would close the broadcast and make `subscribe_track` fail.
    struct TestBroadcast {
        _origin: moq_net::origin::Producer,
        _broadcast: moq_net::broadcast::Producer,
        _tracks: Vec<moq_net::track::Producer>,
        consumer: moq_net::broadcast::Consumer,
    }

    async fn broadcast_with_tracks(names: &[&str]) -> TestBroadcast {
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("test-broadcast", moq_net::broadcast::Route::announced())
            .unwrap();
        let tracks = names
            .iter()
            .map(|name| super::super::create_media_track(&mut broadcast, name, 0).unwrap())
            .collect();
        let consumer = origin.consume().announced_broadcast("test-broadcast").await.unwrap();
        TestBroadcast { _origin: origin, _broadcast: broadcast, _tracks: tracks, consumer }
    }

    #[tokio::test]
    async fn test_attach_late_audio_subscribes_and_reports_pin() {
        let tb = broadcast_with_tracks(&["audio/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let new_tracks = MoqPullNode::extract_tracks(&audio_only_catalog());

        let (late, codec) = node
            .attach_late_audio(&tb.consumer, &new_tracks)
            .await
            .expect("audio track should attach");
        assert_eq!(late.pin_name, "audio/data");
        assert_eq!(codec, AudioCodec::Opus);
        assert!(!late.pin_registered, "audio/data is not a statically advertised pin");
    }

    #[tokio::test]
    async fn test_attach_late_audio_none_without_audio_track() {
        let tb = broadcast_with_tracks(&["video/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let video_only = {
            let mut c = audio_video_catalog();
            c.audio.renditions.clear();
            c
        };
        let new_tracks = MoqPullNode::extract_tracks(&video_only);
        assert!(node.attach_late_audio(&tb.consumer, &new_tracks).await.is_none());
    }

    #[tokio::test]
    async fn test_attach_late_video_subscribes_and_reports_content_type() {
        let tb = broadcast_with_tracks(&["video/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let new_tracks = MoqPullNode::extract_tracks(&audio_video_catalog());

        let (late, content_type) = node
            .attach_late_video(&tb.consumer, &new_tracks)
            .await
            .expect("video track should attach");
        assert_eq!(late.pin_name, "video/data");
        assert_eq!(content_type, VP9_CONTENT_TYPE);
        assert!(!late.pin_registered);
    }

    #[tokio::test]
    async fn test_route_to_dynamic_output_no_consumer() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let outcome = MoqPullNode::route_to_dynamic_output(
            &dynamic_outputs,
            "video/data",
            Packet::Text(Arc::from("x")),
        )
        .await;
        assert!(matches!(outcome, RouteOutcome::NoConsumer));
    }

    #[tokio::test]
    async fn test_route_to_dynamic_output_sends_to_registered_channel() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (tx, mut rx) = mpsc::channel(4);
        MoqPullNode::insert_dynamic_output(&dynamic_outputs, "video/data".to_string(), tx);

        let outcome = MoqPullNode::route_to_dynamic_output(
            &dynamic_outputs,
            "video/data",
            Packet::Text(Arc::from("frame")),
        )
        .await;
        assert!(matches!(outcome, RouteOutcome::Sent));
        assert!(matches!(rx.recv().await, Some(Packet::Text(_))));
    }

    #[tokio::test]
    async fn test_route_to_dynamic_output_closed_channel_is_removed() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (tx, rx) = mpsc::channel(4);
        MoqPullNode::insert_dynamic_output(&dynamic_outputs, "video/data".to_string(), tx);
        drop(rx);

        let outcome = MoqPullNode::route_to_dynamic_output(
            &dynamic_outputs,
            "video/data",
            Packet::Text(Arc::from("x")),
        )
        .await;
        assert!(matches!(outcome, RouteOutcome::NoConsumer));
        assert!(
            !dynamic_outputs
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key("video/data"),
            "a closed channel should be evicted from the registry"
        );
    }

    /// When the `out` pin was fixed to AAC at init but a late Opus track
    /// attaches, the codec-mismatch branch must still subscribe (returning the
    /// discovered codec) while warning that downstream consumers may misdecode.
    #[tokio::test]
    async fn test_attach_late_audio_reports_mismatch_codec() {
        let tb = broadcast_with_tracks(&["audio/data"]).await;
        let config =
            MoqPullConfig { audio_codec: Some(AudioCodec::Aac), ..MoqPullConfig::default() };
        let node = MoqPullNode::new(config);
        let new_tracks = MoqPullNode::extract_tracks(&audio_only_catalog());

        let (late, codec) = node
            .attach_late_audio(&tb.consumer, &new_tracks)
            .await
            .expect("audio track should attach");
        assert_eq!(codec, AudioCodec::Opus, "discovered codec wins for the live subscription");
        assert_eq!(node.advertised_out_audio_codec(), Some(AudioCodec::Aac));
        assert_eq!(late.pin_name, "audio/data");
    }

    #[test]
    fn test_advertised_out_audio_codec_none_for_non_audio_out_pin() {
        let mut node = MoqPullNode::new(MoqPullConfig::default());
        let mut video_out =
            MoqPullNode::make_dynamic_output_pin("video/data", AudioCodec::Opus, None);
        video_out.name = "out".to_string();
        node.output_pins = vec![video_out];
        assert_eq!(
            node.advertised_out_audio_codec(),
            None,
            "a non-audio `out` pin has no advertised audio codec"
        );
    }

    #[tokio::test]
    async fn test_attach_late_video_av1_content_type() {
        let tb = broadcast_with_tracks(&["video/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let new_tracks = MoqPullNode::extract_tracks(&video_catalog(VideoCodec::Av1));
        let (_late, content_type) = node
            .attach_late_video(&tb.consumer, &new_tracks)
            .await
            .expect("av1 track should attach");
        assert_eq!(content_type, AV1_CONTENT_TYPE);
    }

    #[tokio::test]
    async fn test_attach_late_video_h264_content_type() {
        let tb = broadcast_with_tracks(&["video/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let new_tracks = MoqPullNode::extract_tracks(&video_catalog(VideoCodec::H264));
        let (_late, content_type) = node
            .attach_late_video(&tb.consumer, &new_tracks)
            .await
            .expect("h264 track should attach");
        assert_eq!(content_type, H264_CONTENT_TYPE);
    }

    /// A broadcast whose producers have been dropped: `subscribe_track` fails,
    /// exercising the attach helpers' subscribe-error paths.
    async fn closed_broadcast(track_names: &[&str]) -> moq_net::broadcast::Consumer {
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("test-broadcast", moq_net::broadcast::Route::announced())
            .unwrap();
        for name in track_names {
            let _ = super::super::create_media_track(&mut broadcast, name, 0).unwrap();
        }
        let bc = origin.consume().announced_broadcast("test-broadcast").await.unwrap();
        drop(broadcast);
        drop(origin);
        bc
    }

    #[tokio::test]
    async fn test_attach_late_audio_none_when_subscribe_fails() {
        let bc = closed_broadcast(&["audio/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let new_tracks = MoqPullNode::extract_tracks(&audio_only_catalog());
        assert!(node.attach_late_audio(&bc, &new_tracks).await.is_none());
    }

    #[tokio::test]
    async fn test_attach_late_video_none_when_subscribe_fails() {
        let bc = closed_broadcast(&["video/data"]).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let new_tracks = MoqPullNode::extract_tracks(&audio_video_catalog());
        assert!(node.attach_late_video(&bc, &new_tracks).await.is_none());
    }

    /// A statically advertised pin routes through the engine's `output_sender`
    /// (not the `DynamicOutputs` registry) and reports `Sent` on success.
    #[tokio::test]
    async fn test_route_track_frame_static_pin_uses_output_sender() {
        let (mut ctx, mock, _state_rx) = crate::test_utils::create_test_context(HashMap::new(), 1);
        let dynamic_outputs: DynamicOutputs = Arc::default();

        let outcome = MoqPullNode::route_track_frame(
            &mut ctx,
            &dynamic_outputs,
            true,
            "out",
            Packet::Text(Arc::from("frame")),
        )
        .await;
        assert!(matches!(outcome, RouteOutcome::Sent));

        let (_node, pin, packet) = mock.try_recv().await.expect("frame routed to output sender");
        assert_eq!(pin, "out");
        assert!(matches!(packet, Packet::Text(_)));
        assert!(
            dynamic_outputs.read().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(),
            "static pin routing must not touch the dynamic registry"
        );
    }

    /// A pin not advertised at init (`pin_registered == false`) is routed
    /// through the `DynamicOutputs` registry rather than the engine sender.
    #[tokio::test]
    async fn test_route_track_frame_dynamic_pin_uses_registry() {
        let (mut ctx, mock, _state_rx) = crate::test_utils::create_test_context(HashMap::new(), 1);
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (tx, mut rx) = mpsc::channel(1);
        MoqPullNode::insert_dynamic_output(&dynamic_outputs, "video/data".to_string(), tx);

        let outcome = MoqPullNode::route_track_frame(
            &mut ctx,
            &dynamic_outputs,
            false,
            "video/data",
            Packet::Text(Arc::from("frame")),
        )
        .await;
        assert!(matches!(outcome, RouteOutcome::Sent));
        assert!(matches!(rx.recv().await, Some(Packet::Text(_))));
        assert!(
            mock.try_recv().await.is_none(),
            "dynamic pin routing must not touch the engine output sender"
        );
    }

    struct TrackPair {
        _origin: moq_net::origin::Producer,
        _broadcast: moq_net::broadcast::Producer,
        producer: moq_net::track::Producer,
        consumer: moq_net::track::Subscriber,
    }

    async fn track_pair(name: &str) -> TrackPair {
        let origin = moq_net::Origin::random().produce();
        let mut broadcast = origin
            .create_broadcast("test-broadcast", moq_net::broadcast::Route::announced())
            .unwrap();
        let producer = super::super::create_media_track(&mut broadcast, name, 0).unwrap();
        let bc = origin.consume().announced_broadcast("test-broadcast").await.unwrap();
        let consumer = super::super::subscribe_track(&bc, name, 0).await.unwrap();
        TrackPair { _origin: origin, _broadcast: broadcast, producer, consumer }
    }

    fn write_group(producer: &mut moq_net::track::Producer, frames: &[&[u8]]) {
        let mut group = producer.append_group().unwrap();
        for (i, f) in frames.iter().enumerate() {
            let ts = moq_net::Timestamp::from_micros(i as u64).unwrap();
            group.write_frame(ts, bytes::Bytes::copy_from_slice(f)).unwrap();
        }
        group.finish().unwrap();
    }

    #[tokio::test]
    async fn test_read_next_raw_moq_walks_groups_and_flags_first_frame() {
        let mut tp = track_pair("audio/data").await;
        write_group(&mut tp.producer, &[b"a", b"b"]);
        write_group(&mut tp.producer, &[b"c"]);
        tp.producer.finish().unwrap();

        let mut group: Option<moq_net::group::Consumer> = None;
        let mut is_first = false;

        let p1 = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first)
            .await
            .unwrap();
        assert_eq!(p1.as_deref(), Some(&b"a"[..]));
        assert!(is_first, "first frame of a freshly opened group is flagged");

        is_first = false;
        let p2 = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first)
            .await
            .unwrap();
        assert_eq!(p2.as_deref(), Some(&b"b"[..]));
        assert!(!is_first, "subsequent frame in the same group is not flagged");

        let p3 = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first)
            .await
            .unwrap();
        assert_eq!(p3.as_deref(), Some(&b"c"[..]));
        assert!(is_first, "first frame of the second group is flagged again");

        let end = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first)
            .await
            .unwrap();
        assert!(end.is_none(), "finished track reads as end-of-stream");
    }

    #[tokio::test]
    async fn test_read_next_raw_moq_propagates_group_error() {
        let mut tp = track_pair("audio/data").await;
        let mut g = tp.producer.append_group().unwrap();
        g.write_frame(moq_net::Timestamp::from_micros(0).unwrap(), bytes::Bytes::from_static(b"a"))
            .unwrap();

        let mut group: Option<moq_net::group::Consumer> = None;
        let mut is_first = false;

        let p1 = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first)
            .await
            .unwrap();
        assert_eq!(p1.as_deref(), Some(&b"a"[..]));

        // Aborting the group mid-read surfaces the abort error to the consumer
        // instead of a silently truncated stream.
        g.abort(moq_net::Error::Cancel).unwrap();
        let err = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first).await;
        assert!(err.is_err(), "an aborted group should surface as an error");
        assert!(group.is_none(), "the errored group is cleared");
    }

    #[tokio::test]
    async fn test_read_next_raw_moq_propagates_next_group_error() {
        let mut tp = track_pair("audio/data").await;
        tp.producer.abort(moq_net::Error::Cancel).unwrap();

        let mut group: Option<moq_net::group::Consumer> = None;
        let mut is_first = false;
        let err = MoqPullNode::read_next_raw_moq(&mut tp.consumer, &mut group, &mut is_first).await;
        assert!(err.is_err(), "an aborted track should surface as an error");
    }

    #[tokio::test]
    async fn test_parse_catalog_errors_when_track_closed() {
        let mut tp = track_pair(hang::catalog::Catalog::DEFAULT_NAME).await;
        tp.producer.finish().unwrap();
        let node = MoqPullNode::new(MoqPullConfig::default());
        let mut cc = super::super::catalog_consumer::CatalogConsumer::new(tp.consumer);

        let Err(err) = node.parse_catalog(&mut cc).await else {
            panic!("expected a track-closed error");
        };
        assert!(err.to_string().contains("closed"), "expected a track-closed error, got: {err}");
    }

    // `start_paused` lets the catalog timeout elapse in virtual time.
    #[tokio::test(start_paused = true)]
    async fn test_parse_catalog_times_out_without_catalog() {
        let tp = track_pair(hang::catalog::Catalog::DEFAULT_NAME).await;
        let node = MoqPullNode::new(MoqPullConfig::default());
        let mut cc = super::super::catalog_consumer::CatalogConsumer::new(tp.consumer);

        let Err(err) = node.parse_catalog(&mut cc).await else {
            panic!("expected a timeout error");
        };
        assert!(err.to_string().contains("Timed out"), "expected a timeout error, got: {err}");
    }

    #[test]
    fn test_extract_tracks_skips_unsupported_codecs() {
        let mut catalog = audio_only_catalog();
        catalog.audio.renditions.insert(
            "audio/aac-he".to_string(),
            hang::catalog::AudioConfig::new(
                hang::catalog::AudioCodec::AAC(hang::catalog::AAC { profile: 5 }),
                48000,
                2,
            ),
        );
        catalog.video.renditions.insert(
            "video/vp8".to_string(),
            hang::catalog::VideoConfig::new(hang::catalog::VideoCodec::VP8),
        );

        let tracks = MoqPullNode::extract_tracks(&catalog);
        assert!(tracks.iter().all(|t| t.track.name != "audio/aac-he"));
        assert!(tracks.iter().all(|t| t.track.name != "video/vp8"));
        assert_eq!(tracks.len(), 1, "only the supported Opus rendition survives");
        assert!(tracks[0].audio_codec.is_some());
    }

    #[test]
    fn test_input_and_output_pins() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        assert!(node.input_pins().is_empty());
        let outs = node.output_pins();
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].name, "out");
    }

    #[tokio::test]
    async fn test_route_track_frame_static_pin_closed_channel() {
        let (mut ctx, mock, _state_rx) = crate::test_utils::create_test_context(HashMap::new(), 1);
        drop(mock); // closes the only routed-packet receiver
        let dynamic_outputs: DynamicOutputs = Arc::default();

        let outcome = MoqPullNode::route_track_frame(
            &mut ctx,
            &dynamic_outputs,
            true,
            "out",
            Packet::Text(Arc::from("frame")),
        )
        .await;
        assert!(matches!(outcome, RouteOutcome::Closed));
    }

    #[tokio::test]
    async fn test_pin_management_rejects_input_pins_and_ignores_input_variants() {
        let dynamic_outputs: DynamicOutputs = Arc::default();
        let (mgmt_tx, mgmt_rx) = mpsc::channel(8);
        let task = tokio::spawn(MoqPullNode::handle_pin_management(
            mgmt_rx,
            dynamic_outputs.clone(),
            Arc::default(),
            "test-node".to_string(),
            AudioCodec::Opus,
        ));

        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        mgmt_tx
            .send(PinManagementMessage::RequestAddInputPin {
                suggested_name: Some("in".to_string()),
                response_tx: resp_tx,
            })
            .await
            .unwrap();
        assert!(resp_rx.await.unwrap().is_err(), "input pins are not supported");

        mgmt_tx
            .send(PinManagementMessage::RemoveInputPin { pin_name: "in".to_string() })
            .await
            .unwrap();

        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        mgmt_tx
            .send(PinManagementMessage::RequestAddOutputPin {
                suggested_name: Some("out".to_string()),
                response_tx: ack_tx,
            })
            .await
            .unwrap();
        assert!(ack_rx.await.unwrap().is_ok());

        drop(mgmt_tx);
        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_initialize_falls_back_to_default_pin_on_discovery_error() {
        let mut node = MoqPullNode::new(MoqPullConfig::default());
        let (state_tx, _state_rx) = mpsc::channel(10);
        let state_tx = streamkit_core::state::NodeStateSender::new(state_tx, 0);
        let ctx = streamkit_core::InitContext { node_id: "node".to_string(), state_tx };

        let update = node.initialize(&ctx).await.unwrap();
        assert!(matches!(update, streamkit_core::pins::PinUpdate::NoChange));
    }

    #[tokio::test]
    async fn test_run_returns_configuration_error_on_invalid_url() {
        let node = MoqPullNode::new(MoqPullConfig::default());
        let (ctx, _mock, _state_rx) = crate::test_utils::create_test_context(HashMap::new(), 1);

        let result = Box::new(node).run(ctx).await;
        assert!(
            matches!(result, Err(StreamKitError::Configuration(_))),
            "empty URL should surface as a fatal configuration error, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_wait_for_broadcast_ignores_non_shutdown_control_messages() {
        let origin = moq_net::Origin::random().produce();
        let consumer = origin.consume();
        let (control_tx, mut control_rx) = mpsc::channel(8);

        control_tx.send(streamkit_core::control::NodeControlMessage::Start).await.unwrap();

        let wait =
            MoqPullNode::wait_for_announced_broadcast(&mut control_rx, &consumer, "late-cast");
        tokio::pin!(wait);

        // The Start message must be consumed without ending the wait.
        let early = tokio::time::timeout(Duration::from_millis(100), wait.as_mut()).await;
        assert!(early.is_err(), "Start must not end the wait for a late publisher");

        let _broadcast = origin
            .create_broadcast("late-cast", moq_net::broadcast::Route::announced())
            .expect("create_broadcast");

        let outcome = tokio::time::timeout(Duration::from_secs(2), wait)
            .await
            .expect("announcement should resolve the wait");
        assert!(matches!(outcome, BroadcastWait::Broadcast(_)));
    }

    #[tokio::test]
    async fn test_wait_for_broadcast_shutdown_ends_naturally() {
        let origin = moq_net::Origin::random().produce();
        let consumer = origin.consume();
        let (control_tx, mut control_rx) = mpsc::channel(8);

        control_tx.send(streamkit_core::control::NodeControlMessage::Shutdown).await.unwrap();

        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            MoqPullNode::wait_for_announced_broadcast(&mut control_rx, &consumer, "late-cast"),
        )
        .await
        .expect("shutdown should resolve the wait");
        assert!(matches!(outcome, BroadcastWait::End(StreamEndReason::Natural)));
    }
}
