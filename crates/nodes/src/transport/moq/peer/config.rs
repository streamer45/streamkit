// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use streamkit_core::timing::MediaClock;
use streamkit_core::types::{AudioCodec, Packet, PacketType, VideoCodec};
use tokio::sync::{broadcast, mpsc, watch, Semaphore};

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct NodeStatsDelta {
    pub received: u64,
    pub sent: u64,
    pub discarded: u64,
    pub errored: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MediaKind {
    Audio,
    Video,
}

#[derive(Clone, Debug)]
pub(super) struct BroadcastFrame {
    pub data: bytes::Bytes,
    pub duration_us: Option<u64>,
    pub kind: MediaKind,
    pub keyframe: bool,
}

/// Media type state shared from the main select loop to subscriber tasks via a
/// [`watch`] channel.  Subscribers wait for `resolved == true` before building
/// the MoQ catalog so that dynamic pipelines (where `input_types` is empty)
/// don't advertise an empty catalog.
#[derive(Clone, Debug)]
pub(super) struct MediaTypeState {
    pub has_audio: bool,
    pub has_video: bool,
    /// `true` once the media kind of every connected input pin has been
    /// determined — either from `NodeContext::input_types` (static pipelines)
    /// or from the first packet on each pin (dynamic pipelines).
    pub resolved: bool,
}

/// Result of processing a single frame
pub(super) enum FrameResult {
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
pub(super) enum TrackExit {
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
    Error(streamkit_core::StreamKitError),
}

#[derive(Debug)]
pub(super) enum PublisherEvent {
    Connected { path: String },
    Disconnected { path: String, error: Option<String> },
}

/// Resolved codec pair for video and audio.
///
/// Bundles the two codec fields that would otherwise be passed as separate
/// parameters to `handle_pin_management` and friends.
#[derive(Debug, Clone, Copy)]
pub(super) struct MediaCodecConfig {
    pub video: VideoCodec,
    pub audio: AudioCodec,
}

/// Media and output configuration shared across subscriber-related functions.
pub(super) struct SubscriberMediaConfig {
    pub has_video: bool,
    pub has_audio: bool,
    pub video_width: u32,
    pub video_height: u32,
    pub output_group_duration_ms: u64,
    pub output_initial_delay_ms: u64,
    pub video_codec: VideoCodec,
    pub audio_codec: AudioCodec,
}

pub(super) struct BidirectionalTaskConfig {
    /// All input broadcast names — first is primary, rest are additional.
    pub input_broadcasts: Vec<String>,
    pub output_broadcast: String,
    pub node_id: String,
    pub broadcast_rx: broadcast::Receiver<BroadcastFrame>,
    pub shutdown_rx: broadcast::Receiver<()>,
    pub publisher_slot: Arc<Semaphore>,
    pub publisher_events: mpsc::UnboundedSender<PublisherEvent>,
    pub subscriber_count: Arc<std::sync::atomic::AtomicU64>,
    pub media: SubscriberMediaConfig,
    pub media_state_rx: watch::Receiver<MediaTypeState>,
    pub routing: TrackRouting,
}

pub(super) struct PublisherReceiveLoopWithSlotConfig {
    pub subscribe: moq_lite::OriginConsumer,
    pub broadcast_name: String,
    pub publisher_slot: Arc<Semaphore>,
    pub publisher_events: mpsc::UnboundedSender<PublisherEvent>,
    pub publisher_path: String,
    pub routing: TrackRouting,
}

/// Everything a publisher-side track processor needs to route frames and
/// label pins, bundled so the receive-loop call chain doesn't pass five
/// parameters through every layer.
#[derive(Clone)]
pub(super) struct TrackRouting {
    pub output_sender: streamkit_core::OutputSender,
    pub stats_delta_tx: mpsc::Sender<NodeStatsDelta>,
    pub dynamic_outputs: DynamicOutputs,
    pub discovered_codecs: crate::transport::moq::discovered::DiscoveredCodecs,
    /// Local video codec config — fallback for frame `content_type` when the
    /// catalog hasn't advertised a codec for the track.
    pub video_codec: VideoCodec,
}

/// Result of sending a frame to a subscriber
pub(super) enum SendResult {
    /// Continue sending more frames
    Continue,
    /// Stop the send loop
    Stop,
}

/// Bundles the mutable loop state and immutable config that
/// [`super::MoqPeerNode::handle_broadcast_recv`] needs, replacing a 14-parameter
/// function signature with a single context reference.
pub(super) struct SubscriberSendCtx<'a> {
    pub audio_track_producer:
        &'a mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
    pub video_track_producer:
        &'a mut Option<crate::transport::moq::ordered_producer::OrderedProducer>,
    pub packet_count: u64,
    pub frame_count: u64,
    /// Tracks whether the first audio frame has been sent so the initial
    /// MoQ group is opened independently of video frame ordering.
    pub audio_first_sent: bool,
    pub last_log: std::time::Instant,
    pub group_duration_ms: u64,
    pub audio_clock: MediaClock,
    pub video_clock: MediaClock,
    pub gap_histogram: opentelemetry::metrics::Histogram<f64>,
    pub metric_labels: [opentelemetry::KeyValue; 2],
    pub last_audio_ts_ms: Option<u64>,
    pub last_video_ts_ms: Option<u64>,
    pub stats_delta_tx: &'a mpsc::Sender<NodeStatsDelta>,
    /// Resolved audio codec — used for codec-aware default frame durations.
    pub audio_codec: AudioCodec,
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
/// and writes.
pub(super) type DynamicOutputs = Arc<std::sync::RwLock<HashMap<String, mpsc::Sender<Packet>>>>;

pub(super) fn normalize_gateway_path(path: &str) -> String {
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

pub(super) fn join_gateway_path(base: &str, suffix: &str) -> String {
    if base == "/" {
        format!("/{suffix}")
    } else {
        format!("{base}/{suffix}")
    }
}

/// Infer the [`MediaKind`] from a [`PacketType`].
///
/// Returns `Some(Audio)` for encoded audio, `Some(Video)` for encoded video,
/// and `None` for anything else.
pub(super) const fn media_kind_for_packet_type(pt: &PacketType) -> Option<MediaKind> {
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
pub(super) fn infer_kind_from_packet(packet: &Packet) -> MediaKind {
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
pub(super) fn make_broadcast_frame(packet: Packet, kind: MediaKind) -> Option<BroadcastFrame> {
    if let Packet::Binary { data, metadata, .. } = packet {
        let duration_us = super::super::constants::packet_duration_us(metadata.as_ref());
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

#[derive(Deserialize, Debug, JsonSchema, Clone)]
#[serde(default, deny_unknown_fields)]
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
    /// Video codec for the MoQ catalog.
    ///
    /// Required for dynamic pipelines where `input_types` is not available at
    /// startup.  When `None`, the codec is auto-detected from `input_types`
    /// (static pipelines) and falls back to VP9.
    pub video_codec: Option<VideoCodec>,
    /// Audio codec for the MoQ catalog.
    ///
    /// Required for dynamic pipelines where `input_types` is not available at
    /// startup.  When `None`, the codec is auto-detected from `input_types`
    /// (static pipelines) and falls back to Opus.
    ///
    /// Controls the **publisher output pin** type (`audio/data`).  For
    /// transcoding scenarios where the subscriber receives a different codec
    /// (e.g. Opus in → AAC out), use [`subscriber_audio_codec`] to override
    /// the subscriber catalog codec independently.
    pub audio_codec: Option<AudioCodec>,
    /// Audio codec advertised in the **subscriber** MoQ catalog.
    ///
    /// When set, overrides [`audio_codec`] for the subscriber side only
    /// (catalog, frame duration).  The publisher output pin (`audio/data`)
    /// continues to use [`audio_codec`].
    ///
    /// Useful for transcoding pipelines where the publisher sends one codec
    /// (e.g. Opus) but the pipeline re-encodes to another (e.g. AAC) before
    /// feeding it back to subscribers.
    ///
    /// When `None`, falls back to [`audio_codec`].
    pub subscriber_audio_codec: Option<AudioCodec>,
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
            video_codec: None,
            audio_codec: None,
            subscriber_audio_codec: None,
        }
    }
}

impl MoqPeerConfig {
    /// The primary (first) input broadcast name.
    ///
    /// Falls back to `"input"` if the vec is empty, but this should never
    /// happen in practice because [`super::MoqPeerNode::run`] validates that
    /// `input_broadcasts` is non-empty at startup.
    pub(super) fn primary_input_broadcast(&self) -> &str {
        self.input_broadcasts.first().map_or("input", |s| s.as_str())
    }

    /// Additional input broadcast names beyond the primary.
    pub(super) fn extra_input_broadcasts(&self) -> &[String] {
        if self.input_broadcasts.len() > 1 {
            &self.input_broadcasts[1..]
        } else {
            &[]
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn normalize_gateway_path_default() {
        assert_eq!(normalize_gateway_path("/moq"), "/moq");
    }

    #[test]
    fn normalize_gateway_path_strips_trailing_slash() {
        assert_eq!(normalize_gateway_path("/moq/"), "/moq");
    }

    #[test]
    fn normalize_gateway_path_adds_leading_slash() {
        assert_eq!(normalize_gateway_path("moq"), "/moq");
    }

    #[test]
    fn normalize_gateway_path_empty_defaults_to_moq() {
        assert_eq!(normalize_gateway_path(""), "/moq");
    }

    #[test]
    fn normalize_gateway_path_whitespace_defaults_to_moq() {
        assert_eq!(normalize_gateway_path("   "), "/moq");
    }

    #[test]
    fn normalize_gateway_path_root_slash() {
        assert_eq!(normalize_gateway_path("/"), "/");
    }

    #[test]
    fn normalize_gateway_path_nested() {
        assert_eq!(normalize_gateway_path("/api/moq/"), "/api/moq");
    }

    #[test]
    fn join_gateway_path_normal() {
        assert_eq!(join_gateway_path("/moq", "input"), "/moq/input");
    }

    #[test]
    fn join_gateway_path_root_base() {
        assert_eq!(join_gateway_path("/", "output"), "/output");
    }

    #[test]
    fn join_gateway_path_nested_base() {
        assert_eq!(join_gateway_path("/api/moq", "input"), "/api/moq/input");
    }

    #[test]
    fn media_kind_audio() {
        let pt = PacketType::EncodedAudio(streamkit_core::types::EncodedAudioFormat {
            codec: streamkit_core::types::AudioCodec::Opus,
            codec_private: None,
        });
        assert_eq!(media_kind_for_packet_type(&pt), Some(MediaKind::Audio));
    }

    #[test]
    fn media_kind_video() {
        let pt = PacketType::EncodedVideo(streamkit_core::types::EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        });
        assert_eq!(media_kind_for_packet_type(&pt), Some(MediaKind::Video));
    }

    #[test]
    fn media_kind_binary_returns_none() {
        assert_eq!(media_kind_for_packet_type(&PacketType::Binary), None);
    }

    #[test]
    fn infer_kind_video_content_type() {
        let packet = Packet::Binary {
            data: bytes::Bytes::from_static(b"frame"),
            content_type: Some(std::borrow::Cow::Borrowed("video/vp9")),
            metadata: None,
        };
        assert_eq!(infer_kind_from_packet(&packet), MediaKind::Video);
    }

    #[test]
    fn infer_kind_audio_content_type() {
        let packet = Packet::Binary {
            data: bytes::Bytes::from_static(b"frame"),
            content_type: Some(std::borrow::Cow::Borrowed("audio/opus")),
            metadata: None,
        };
        assert_eq!(infer_kind_from_packet(&packet), MediaKind::Audio);
    }

    #[test]
    fn infer_kind_no_content_type_defaults_audio() {
        let packet = Packet::Binary {
            data: bytes::Bytes::from_static(b"frame"),
            content_type: None,
            metadata: None,
        };
        assert_eq!(infer_kind_from_packet(&packet), MediaKind::Audio);
    }

    #[test]
    fn make_broadcast_frame_video_defaults_keyframe_true() {
        let packet = Packet::Binary {
            data: bytes::Bytes::from_static(b"video-data"),
            content_type: None,
            metadata: None,
        };
        let frame = make_broadcast_frame(packet, MediaKind::Video).unwrap();
        assert!(frame.keyframe, "video frame without metadata should default to keyframe=true");
        assert_eq!(frame.kind, MediaKind::Video);
    }

    #[test]
    fn make_broadcast_frame_audio_never_keyframe() {
        let packet = Packet::Binary {
            data: bytes::Bytes::from_static(b"audio-data"),
            content_type: None,
            metadata: None,
        };
        let frame = make_broadcast_frame(packet, MediaKind::Audio).unwrap();
        assert!(!frame.keyframe, "audio frames should never be marked as keyframe");
        assert_eq!(frame.kind, MediaKind::Audio);
    }

    #[test]
    fn make_broadcast_frame_returns_none_for_non_binary() {
        let packet = Packet::Text(std::sync::Arc::from("not binary"));
        assert!(make_broadcast_frame(packet, MediaKind::Audio).is_none());
    }

    #[test]
    fn config_primary_input_broadcast_default() {
        let config = MoqPeerConfig::default();
        assert_eq!(config.primary_input_broadcast(), "input");
    }

    #[test]
    fn config_primary_input_broadcast_custom() {
        let config = MoqPeerConfig {
            input_broadcasts: vec!["cam".to_string(), "screen".to_string()],
            ..MoqPeerConfig::default()
        };
        assert_eq!(config.primary_input_broadcast(), "cam");
    }

    #[test]
    fn config_extra_input_broadcasts_single() {
        let config = MoqPeerConfig::default();
        assert!(config.extra_input_broadcasts().is_empty());
    }

    #[test]
    fn config_extra_input_broadcasts_multiple() {
        let config = MoqPeerConfig {
            input_broadcasts: vec!["cam".to_string(), "screen".to_string(), "aux".to_string()],
            ..MoqPeerConfig::default()
        };
        assert_eq!(config.extra_input_broadcasts(), &["screen", "aux"]);
    }

    #[test]
    fn config_primary_input_broadcast_empty_vec_fallback() {
        let config = MoqPeerConfig { input_broadcasts: vec![], ..MoqPeerConfig::default() };
        assert_eq!(config.primary_input_broadcast(), "input");
    }
}
