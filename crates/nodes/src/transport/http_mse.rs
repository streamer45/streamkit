// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use bytes::Bytes;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    config_helpers, state_helpers, stats::NodeStatsTracker, InputPin, NodeContext, OutputPin,
    PinCardinality, ProcessorNode, StreamKitError,
};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HttpMseConfig {
    /// Path suffix for the MSE stream endpoint (e.g., "/video").
    /// Full URL will be: `/mse/{session_id}{path}`
    pub path: String,
    /// Maximum concurrent HTTP clients (default: 10).
    #[serde(default = "default_max_clients")]
    #[schemars(range(min = 1))]
    pub max_clients: u32,
    /// Content type for the HTTP response.
    /// Defaults to `video/webm; codecs="vp9,opus"`.
    /// For best MSE compatibility, include the codecs parameter
    /// (e.g., `video/webm; codecs="vp9,opus"` or `video/webm; codecs="vp9"`).
    /// For fragmented-MP4 input set this to the matching `video/mp4` type
    /// (e.g., `video/mp4; codecs="avc1.640028,mp4a.40.2"`); the container
    /// format itself is auto-detected from the stream.
    #[serde(default)]
    pub content_type: Option<String>,
}

const fn default_max_clients() -> u32 {
    10
}

/// Default content type for WebM MSE streams.
/// Includes the codecs parameter for best browser compatibility with MSE.
const DEFAULT_CONTENT_TYPE: &str = "video/webm; codecs=\"vp9,opus\"";

/// Maximum size for the init segment buffer (16 KB).
/// WebM init segments are typically < 1 KB; this is a generous upper bound.
const MAX_INIT_SEGMENT_SIZE: usize = 16 * 1024;

/// WebM Cluster element ID (0x1F43B675).
/// Used to detect when the init segment ends and media data begins.
const WEBM_CLUSTER_ID: [u8; 4] = [0x1F, 0x43, 0xB6, 0x75];

/// WebM Tracks element ID (0x1654AE6B).
/// Used to find the end of the header (EBML + Segment + Info + Tracks)
/// so the init segment can be truncated before any SimpleBlock data
/// that the muxer may emit prior to opening the first Cluster.
const WEBM_TRACKS_ID: [u8; 4] = [0x16, 0x54, 0xAE, 0x6B];

/// EBML header magic (0x1A45DFA3) — first bytes of every WebM/Matroska stream.
const EBML_HEADER_ID: [u8; 4] = [0x1A, 0x45, 0xDF, 0xA3];

/// ISO-BMFF `ftyp` box type — the first box of an fMP4 init segment.
const FMP4_FTYP: [u8; 4] = *b"ftyp";

/// ISO-BMFF `moof` (Movie Fragment) box type — marks the start of each fMP4
/// media segment, the fMP4 analogue of a WebM Cluster.
const FMP4_MOOF: [u8; 4] = *b"moof";

/// Generous cap on the fMP4 init segment (ftyp + moov) buffer.  A single
/// audio+video `moov` is only a few KB; this bounds memory in the pathological
/// case where a `moof` box never arrives.
const MAX_FMP4_INIT_SEGMENT_SIZE: usize = 256 * 1024;

/// Container format of the stream served to MSE clients.  Detected from the
/// first bytes of the upstream muxer's output so the node can locate init
/// segment / media segment boundaries the right way for each format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum MseFormat {
    /// VP8/VP9/AV1-in-WebM — media segments are EBML Clusters.
    WebM,
    /// H.264/AV1-in-fragmented-MP4 — media segments are `moof`+`mdat` boxes.
    Fmp4,
}

/// Detect the container format from the leading bytes of the stream.
///
/// WebM starts with the EBML header magic; fMP4 starts with an `ftyp` box
/// (`[size: u32][b"ftyp"]`).  Returns `None` when the format is not yet
/// recognisable, in which case the caller defaults to WebM for backward
/// compatibility.
fn detect_mse_format(data: &[u8]) -> Option<MseFormat> {
    if data.starts_with(&EBML_HEADER_ID) {
        return Some(MseFormat::WebM);
    }
    if data.len() >= 8 && data[4..8] == FMP4_FTYP {
        return Some(MseFormat::Fmp4);
    }
    None
}

/// Derive the container format from the configured `content_type` MIME string.
///
/// The `content_type` is what the gateway actually serves to clients, so it is
/// the authoritative source of truth for how the stream must be parsed — byte
/// sniffing is only a fallback when the MIME type is ambiguous. Returns `None`
/// for unrecognised MIME types.
fn format_from_content_type(content_type: &str) -> Option<MseFormat> {
    let ct = content_type.to_ascii_lowercase();
    if ct.contains("mp4") {
        Some(MseFormat::Fmp4)
    } else if ct.contains("webm") || ct.contains("matroska") {
        Some(MseFormat::WebM)
    } else {
        None
    }
}

/// Walk the top-level ISO-BMFF boxes in `data` and return the byte offset of
/// the first `moof` (Movie Fragment) box.
///
/// A box is laid out as `[size: u32 BE][type: 4 bytes][body]`.  `size == 1`
/// means a 64-bit `largesize` follows the type; `size == 0` means the box runs
/// to EOF.  Because every non-`moof` box is skipped by its declared size, byte
/// sequences inside an `mdat` payload are never mistaken for a box header —
/// unlike the literal scan used for WebM Clusters.
///
/// Returns `None` when no `moof` is present in the fully-parseable prefix
/// (e.g. the `moov` box is not yet completely buffered), signalling the caller
/// to buffer more data and retry.
fn fmp4_find_first_moof(data: &[u8]) -> Option<usize> {
    let mut offset = 0usize;
    loop {
        if offset + 8 > data.len() {
            return None;
        }

        let size32 = u32::from_be_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ]);

        if data[offset + 4..offset + 8] == FMP4_MOOF {
            return Some(offset);
        }

        let (header_len, box_size) = match size32 {
            1 => {
                if offset + 16 > data.len() {
                    return None;
                }
                let large = u64::from_be_bytes(data[offset + 8..offset + 16].try_into().ok()?);
                (16u64, large)
            },
            // `size == 0` means "to end of file": no further top-level box can
            // follow, so there is no `moof` beyond this point.
            0 => return None,
            n => (8u64, u64::from(n)),
        };

        if box_size < header_len {
            return None; // Malformed: box smaller than its own header.
        }

        let next = (offset as u64).checked_add(box_size)?;
        let next = usize::try_from(next).ok()?;
        if next > data.len() {
            return None; // Box body not fully buffered yet.
        }
        offset = next;
    }
}

/// Hard cap on the rolling GOP buffer to prevent unbounded growth when
/// keyframes are infrequent (e.g. 10 s interval at high bitrate).
const MAX_GOP_BUFFER_SIZE: usize = 2 * 1024 * 1024; // 2 MB

/// Send `chunk` to every connected client, dropping any whose channel is
/// closed or full, and return the number that received it.
///
/// A full channel means the client can't keep up; since dropping bytes
/// mid-stream corrupts the MSE container (the `SourceBuffer` throws an
/// unrecoverable decode error), the slow client is disconnected rather than
/// sent a partial stream.
fn broadcast_to_clients(clients: &mut Vec<mpsc::Sender<Bytes>>, chunk: &Bytes) -> usize {
    let mut delivered = 0usize;
    let mut i = 0;
    while i < clients.len() {
        match clients[i].try_send(chunk.clone()) {
            Ok(()) => {
                delivered += 1;
                i += 1;
            },
            Err(mpsc::error::TrySendError::Closed(_)) => {
                clients.swap_remove(i);
                tracing::debug!(client_count = clients.len(), "MSE client disconnected");
            },
            Err(mpsc::error::TrySendError::Full(_)) => {
                clients.swap_remove(i);
                tracing::warn!(
                    client_count = clients.len(),
                    "MSE client too slow, disconnecting to avoid corrupt stream"
                );
            },
        }
    }
    delivered
}

/// Buffers the WebM init segment and replays it to late-joining HTTP clients.
pub struct HttpMseNode {
    config: HttpMseConfig,
}

impl HttpMseNode {
    pub fn factory() -> streamkit_core::node::NodeFactory {
        std::sync::Arc::new(|params| {
            let config: HttpMseConfig = if params.is_none() {
                // Default config for pin inspection only (dynamic pipelines)
                HttpMseConfig {
                    path: "/video".to_string(),
                    max_clients: default_max_clients(),
                    content_type: None,
                }
            } else {
                config_helpers::parse_config_required(params)?
            };

            if config.max_clients == 0 {
                return Err(StreamKitError::Configuration(
                    "max_clients must be greater than 0".to_string(),
                ));
            }
            if config.path.contains("..") || config.path.contains("//") {
                return Err(StreamKitError::Configuration(
                    "path must not contain '..' or empty segments".to_string(),
                ));
            }

            Ok(Box::new(Self { config }))
        })
    }
}

#[async_trait]
impl ProcessorNode for HttpMseNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Binary],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        // Pure sink node — no outputs.
        vec![]
    }

    #[allow(clippy::too_many_lines)]
    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        // Session ID is required for gateway registration.
        let session_id = context.session_id.as_ref().ok_or_else(|| {
            let err = "transport::http::mse requires a session_id for gateway registration";
            tracing::error!("{}", err);
            StreamKitError::Configuration(err.to_string())
        })?;

        let gateway = streamkit_core::mse_gateway::get_mse_gateway().ok_or_else(|| {
            let err = "MSE gateway not available — ensure transport::http::mse is used in a session with gateway support";
            tracing::error!("{}", err);
            StreamKitError::Runtime(err.to_string())
        })?;

        let content_type =
            self.config.content_type.clone().unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string());

        // The served content_type is the authoritative source of truth for how
        // the stream must be parsed; byte sniffing is only a fallback.
        let configured_format = format_from_content_type(&content_type);

        let path_suffix = if self.config.path.starts_with('/') {
            self.config.path.clone()
        } else {
            format!("/{}", self.config.path)
        };
        let full_path = format!("/mse/{session_id}{path_suffix}");

        tracing::info!(
            path = %full_path,
            session_id = %session_id,
            content_type = %content_type,
            max_clients = self.config.max_clients,
            "HttpMseNode registering MSE stream"
        );

        let mut client_rx = gateway
            .register_stream(
                full_path.clone(),
                session_id.clone(),
                content_type,
                self.config.max_clients,
            )
            .await
            .map_err(|e| {
                let err = format!("Failed to register MSE stream: {e}");
                tracing::error!("{}", err);
                StreamKitError::Runtime(err)
            })?;

        let mut input_rx = context.take_input("in").map_err(|e| {
            tracing::error!("Failed to take input pin: {}", e);
            e
        })?;

        state_helpers::emit_running(&context.state_tx, &node_name);
        let node_start = std::time::Instant::now();

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        let mut clients: Vec<mpsc::Sender<Bytes>> = Vec::new();

        // Init segment: accumulates until the first media-segment boundary
        // (WebM Cluster / fMP4 `moof`), then truncated to the container header
        // (WebM: EBML+Segment+Info+Tracks; fMP4: ftyp+moov).
        // Rolling GOP buffer: data since the most recent segment boundary;
        // late-joining clients receive init + GOP buffer.
        let mut init_segment: Vec<u8> = Vec::new();
        let mut gop_buffer: Vec<u8> = Vec::new();
        let mut init_complete = false;

        // Overlap buffer: last 3 bytes of previous chunk, for detecting
        // a 4-byte Cluster ID that straddles chunk boundaries (WebM only).
        let mut overlap: Vec<u8> = Vec::new();
        let mut overlap_bytes_in_init: usize = 0;

        // Container format: the configured content_type when recognised,
        // otherwise sniffed from the first non-empty packet.
        let mut format: Option<MseFormat> = None;

        // Set once the fMP4 init buffer is frozen at its cap without a moof,
        // so the bounded-memory warning is logged only a single time.
        let mut fmp4_init_capped = false;

        let cancellation_token = context.cancellation_token.clone();

        let result: Result<(), StreamKitError> = loop {
            tokio::select! {
                biased;

                () = async {
                    if let Some(ref token) = cancellation_token {
                        token.cancelled().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    tracing::info!("HttpMseNode shutting down (cancellation)");
                    break Ok(());
                }

                msg = context.control_rx.recv() => {
                    match msg {
                        Some(streamkit_core::control::NodeControlMessage::Shutdown) => {
                            tracing::info!("HttpMseNode received shutdown");
                            break Ok(());
                        }
                        Some(_) => { /* ignore other control messages */ }
                        None => {
                            tracing::debug!("Control channel closed");
                            break Ok(());
                        }
                    }
                }

                client = client_rx.recv() => {
                    let Some(client) = client else {
                        tracing::warn!("MSE gateway client channel closed");
                        break Ok(());
                    };

                    if !init_segment.is_empty() {
                        let init_bytes = Bytes::copy_from_slice(&init_segment);
                        match client.body_tx.try_send(init_bytes) {
                            Ok(()) => {
                                if !gop_buffer.is_empty() {
                                    let gop = Bytes::copy_from_slice(&gop_buffer);
                                    if client.body_tx.try_send(gop).is_err() {
                                        tracing::debug!("New MSE client disconnected before GOP delivery");
                                        continue;
                                    }
                                }
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                tracing::debug!("New MSE client disconnected before init segment delivery");
                                continue;
                            }
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                tracing::warn!("New MSE client channel full during init segment delivery, disconnecting");
                                continue;
                            }
                        }
                    }

                    tracing::debug!(
                        client_count = clients.len() + 1,
                        "New MSE client connected"
                    );
                    clients.push(client.body_tx);
                }

                packet = input_rx.recv() => {
                    let Some(packet) = packet else {
                        tracing::info!("Input channel closed");
                        break Ok(());
                    };

                    stats_tracker.received();

                    let Packet::Binary { data, .. } = packet else {
                        tracing::warn!("HttpMseNode received non-binary packet, ignoring");
                        stats_tracker.discarded();
                        continue;
                    };

                    if data.is_empty() {
                        continue;
                    }

                    let stream_format = *format.get_or_insert_with(|| {
                        let sniffed = detect_mse_format(&data);
                        if let (Some(cfg), Some(det)) = (configured_format, sniffed) {
                            if cfg != det {
                                tracing::warn!(
                                    configured = ?cfg,
                                    sniffed = ?det,
                                    "HTTP MSE: configured content_type disagrees with the stream's bytes; trusting content_type"
                                );
                            }
                        }
                        let chosen = configured_format.or(sniffed).unwrap_or(MseFormat::WebM);
                        tracing::info!(
                            format = ?chosen,
                            from_content_type = configured_format.is_some(),
                            "HTTP MSE: container format selected"
                        );
                        chosen
                    });

                    // Bytes to forward to clients / the rolling GOP buffer for
                    // this packet (empty while the init segment is still being
                    // assembled).
                    let forward_data: Bytes;

                    match stream_format {
                        MseFormat::Fmp4 => {
                            if init_complete {
                                forward_data = data.clone();
                            } else {
                                // Grow the buffer only while it's under the cap.
                                // The first packet usually carries ftyp+moov AND
                                // the first moof+mdat together, so the whole
                                // packet must be appended and searched before any
                                // size guard — truncating first would corrupt the
                                // first media segment. Once the buffer reaches the
                                // cap without a moof, freeze it (stop appending)
                                // rather than truncate-then-append: truncating
                                // leaves a gap that box-walking can never
                                // re-align past, permanently mangling the stream.
                                if init_segment.len() < MAX_FMP4_INIT_SEGMENT_SIZE {
                                    init_segment.extend_from_slice(&data);
                                }

                                let Some(moof_off) = fmp4_find_first_moof(&init_segment) else {
                                    if init_segment.len() >= MAX_FMP4_INIT_SEGMENT_SIZE
                                        && !fmp4_init_capped
                                    {
                                        fmp4_init_capped = true;
                                        tracing::warn!(
                                            size = init_segment.len(),
                                            "HTTP MSE: fMP4 init segment reached {}B before a moof box; freezing buffer (bounded-memory guard) — stream cannot start",
                                            MAX_FMP4_INIT_SEGMENT_SIZE
                                        );
                                    }
                                    continue;
                                };

                                // Everything before the first `moof` is the init
                                // segment (ftyp + moov); the rest is media.
                                forward_data = Bytes::copy_from_slice(&init_segment[moof_off..]);
                                init_segment.truncate(moof_off);
                                init_complete = true;

                                let elapsed_ms = u64::try_from(node_start.elapsed().as_millis())
                                    .unwrap_or(u64::MAX);
                                tracing::info!(
                                    init_segment_size = init_segment.len(),
                                    elapsed_ms,
                                    "fMP4 init segment (ftyp+moov) captured"
                                );

                                if !clients.is_empty() && !init_segment.is_empty() {
                                    let init_bytes = Bytes::copy_from_slice(&init_segment);
                                    broadcast_to_clients(&mut clients, &init_bytes);
                                }
                            }
                        }
                        MseFormat::WebM => {
                            let mut cluster_start_in_data: Option<usize> = None;
                            let mut cluster_prefix: Vec<u8> = Vec::new();

                            if !init_complete {
                                let cluster_found = if overlap.is_empty() {
                                    false
                                } else {
                                    let mut combined = overlap.clone();
                                    combined.extend_from_slice(&data[..data.len().min(WEBM_CLUSTER_ID.len())]);
                                    find_cluster_id(&combined).is_some_and(|pos| {
                                        let overlap_len = overlap.len();
                                        if pos < overlap_len {
                                            // Only truncate bytes that were actually appended to
                                            // init_segment — the overlap may extend past what was
                                            // buffered if the init_segment hit MAX_INIT_SEGMENT_SIZE.
                                            let bytes_to_remove = (overlap_len - pos).min(overlap_bytes_in_init);
                                            init_segment.truncate(init_segment.len() - bytes_to_remove);
                                            cluster_prefix = overlap[pos..].to_vec();
                                            cluster_start_in_data = Some(0);
                                        } else {
                                            let extra = pos - overlap_len;
                                            let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                            let to_append = extra.min(remaining_capacity);
                                            init_segment.extend_from_slice(&data[..to_append]);
                                            cluster_start_in_data = Some(extra);
                                        }
                                        true
                                    })
                                };

                                if cluster_found {
                                    init_complete = true;
                                } else if let Some(cluster_offset) = find_cluster_id(&data) {
                                    if cluster_offset > 0 {
                                        let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                        let to_append = cluster_offset.min(remaining_capacity);
                                        init_segment.extend_from_slice(&data[..to_append]);
                                    }
                                    init_complete = true;
                                    cluster_start_in_data = Some(cluster_offset);
                                } else {
                                    let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                    let appended = if remaining_capacity > 0 {
                                        let to_append = data.len().min(remaining_capacity);
                                        init_segment.extend_from_slice(&data[..to_append]);
                                        to_append
                                    } else {
                                        0
                                    };
                                    let keep = data.len().min(WEBM_CLUSTER_ID.len() - 1);
                                    overlap.clear();
                                    overlap.extend_from_slice(&data[data.len() - keep..]);
                                    overlap_bytes_in_init = appended.saturating_sub(data.len() - keep);
                                }

                                if init_complete {
                                    // Truncate to WebM header (EBML + Segment + Info + Tracks).
                                    // Pre-Cluster SimpleBlock data is invalid for MSE.
                                    if let Some(tracks_end) = find_tracks_end(&init_segment) {
                                        if tracks_end < init_segment.len() {
                                            tracing::debug!(
                                                raw_size = init_segment.len(),
                                                tracks_end,
                                                stripped = init_segment.len() - tracks_end,
                                                "Stripping pre-Cluster SimpleBlock data from init segment"
                                            );
                                            init_segment.truncate(tracks_end);
                                        }
                                    }

                                    let elapsed_ms = u64::try_from(node_start.elapsed().as_millis())
                                        .unwrap_or(u64::MAX);
                                    tracing::info!(
                                        init_segment_size = init_segment.len(),
                                        elapsed_ms,
                                        "WebM init segment captured"
                                    );
                                    overlap.clear();

                                    if !clients.is_empty() && !init_segment.is_empty() {
                                        let init_bytes = Bytes::copy_from_slice(&init_segment);
                                        broadcast_to_clients(&mut clients, &init_bytes);
                                    }
                                }
                            }

                            if !init_complete {
                                continue;
                            }

                            forward_data = match cluster_start_in_data {
                                Some(offset) if offset < data.len() => {
                                    if cluster_prefix.is_empty() {
                                        data.slice(offset..)
                                    } else {
                                        let mut combined = cluster_prefix;
                                        combined.extend_from_slice(&data[offset..]);
                                        Bytes::from(combined)
                                    }
                                }
                                Some(_) => continue,
                                None => data.clone(),
                            };
                        }
                    }

                    if !forward_data.is_empty() {
                        let new_segment = match stream_format {
                            MseFormat::Fmp4 => fmp4_find_first_moof(&forward_data).is_some(),
                            MseFormat::WebM => find_cluster_id(&forward_data).is_some(),
                        };
                        if new_segment {
                            let elapsed_ms = u64::try_from(node_start.elapsed().as_millis())
                                .unwrap_or(u64::MAX);
                            tracing::debug!(
                                gop_size = gop_buffer.len(),
                                elapsed_ms,
                                "HTTP MSE: new media segment (GOP reset)"
                            );
                            gop_buffer.clear();
                        }
                        gop_buffer.extend_from_slice(&forward_data);
                        if gop_buffer.len() > MAX_GOP_BUFFER_SIZE {
                            tracing::warn!(
                                gop_size = gop_buffer.len(),
                                "HTTP MSE: GOP buffer exceeded {}B cap, resetting",
                                MAX_GOP_BUFFER_SIZE
                            );
                            gop_buffer.clear();
                        }
                    }

                    if !clients.is_empty() && !forward_data.is_empty() {
                        // Counts total chunk deliveries across all clients, which
                        // differs from single-output nodes but reflects fan-out.
                        let delivered = broadcast_to_clients(&mut clients, &forward_data);
                        stats_tracker.sent_n(u64::try_from(delivered).unwrap_or(u64::MAX));
                    }

                    stats_tracker.maybe_send();
                }
            }
        };

        // Clean up: unregister from gateway.
        gateway.unregister_stream(&full_path).await;
        clients.clear();

        stats_tracker.force_send();

        match &result {
            Ok(()) => {
                state_helpers::emit_stopped(&context.state_tx, &node_name, "completed");
            },
            Err(e) => {
                state_helpers::emit_failed(&context.state_tx, &node_name, e.to_string());
            },
        }

        result
    }
}

/// WebM Timecode element ID (single byte: `0xE7`).
#[cfg(test)]
const WEBM_TIMECODE_ID: u8 = 0xE7;

/// Find the byte offset immediately after the Cluster preamble in `data`.
///
/// The Cluster preamble consists of:
///   Cluster ID (4 bytes) + size VINT + Timecode element (ID + size VINT + value)
///
/// Late-joining MSE clients need the Cluster preamble prepended before they can
/// decode SimpleBlock data, because MSE requires SimpleBlocks to be inside an
/// open Cluster element.
///
/// `data` must start at the Cluster element ID.  Returns `None` if the
/// preamble is truncated.
#[cfg(test)]
fn find_cluster_preamble_end(data: &[u8]) -> Option<usize> {
    // Skip Cluster ID (4 bytes).
    let mut pos = WEBM_CLUSTER_ID.len();
    if pos >= data.len() {
        return None;
    }

    // Skip Cluster size VINT.
    let first_byte = data[pos];
    if first_byte == 0 {
        return None;
    }
    let width = first_byte.leading_zeros() as usize + 1;
    if pos + width > data.len() || width > 8 {
        return None;
    }
    pos += width;

    // Expect Timecode element (0xE7).
    if pos >= data.len() || data[pos] != WEBM_TIMECODE_ID {
        // No Timecode sub-element — return what we have (ID + size only).
        return Some(pos);
    }
    pos += 1; // skip Timecode ID byte

    // Parse Timecode size VINT.
    if pos >= data.len() {
        return None;
    }
    let tc_first = data[pos];
    if tc_first == 0 {
        return None;
    }
    let tc_width = tc_first.leading_zeros() as usize + 1;
    if pos + tc_width > data.len() || tc_width > 8 {
        return None;
    }
    let tc_mask = u32::try_from(tc_width).ok().and_then(|w| 0xFFu8.checked_shr(w)).unwrap_or(0);
    let mut tc_size: u64 = u64::from(tc_first & tc_mask);
    for &b in &data[pos + 1..pos + tc_width] {
        tc_size = (tc_size << 8) | u64::from(b);
    }
    let tc_content = usize::try_from(tc_size).ok()?;
    pos += tc_width;

    // Skip Timecode content.
    pos = pos.checked_add(tc_content)?;
    if pos > data.len() {
        return None;
    }

    Some(pos)
}

/// Search for the WebM Cluster element ID (0x1F43B675) in a byte slice.
/// Returns the offset of the first occurrence, or `None` if not found.
fn find_cluster_id(data: &[u8]) -> Option<usize> {
    if data.len() < WEBM_CLUSTER_ID.len() {
        return None;
    }
    data.windows(WEBM_CLUSTER_ID.len()).position(|window| window == WEBM_CLUSTER_ID)
}

/// Find the byte offset immediately after the WebM Tracks element in `data`.
///
/// The Tracks element (ID `0x1654AE6B`) is the last header element before
/// media data.  For MSE playback the init segment must contain exactly
/// EBML + Segment + Info + Tracks — nothing more.  This function locates the
/// Tracks element, parses its EBML VINT-encoded size, and returns the offset
/// of the first byte after it.
///
/// Returns `None` if the Tracks element is not found or if the size encoding
/// is truncated (data too short).
fn find_tracks_end(data: &[u8]) -> Option<usize> {
    // Locate the 4-byte Tracks element ID.
    let id_pos = data.windows(WEBM_TRACKS_ID.len()).position(|w| w == WEBM_TRACKS_ID)?;

    let size_start = id_pos + WEBM_TRACKS_ID.len();
    if size_start >= data.len() {
        return None;
    }

    // Parse the EBML VINT (variable-width integer) that encodes the element
    // size.  The number of leading zero bits in the first byte determines the
    // width (1–8 bytes).
    let first_byte = data[size_start];
    if first_byte == 0 {
        return None; // Invalid VINT.
    }
    let width = first_byte.leading_zeros() as usize + 1;
    if size_start + width > data.len() || width > 8 {
        return None; // Truncated or invalid.
    }

    // Assemble the size value, masking out the width marker bits.
    // For width 1–7 the mask is 0xFF >> width; for width 8 the entire
    // first byte is the marker so the mask is 0x00.
    let mask = u32::try_from(width).ok().and_then(|w| 0xFFu8.checked_shr(w)).unwrap_or(0);
    let mut size: u64 = u64::from(first_byte & mask);
    for &b in &data[size_start + 1..size_start + width] {
        size = (size << 8) | u64::from(b);
    }

    // Guard against absurd sizes that would overflow usize.
    let content_size = usize::try_from(size).ok()?;
    let tracks_end = size_start.checked_add(width)?.checked_add(content_size)?;

    Some(tracks_end)
}

/// Register HTTP MSE nodes with the registry.
pub fn register_http_mse_nodes(registry: &mut streamkit_core::NodeRegistry) {
    register_dynamic_node!(
        registry,
        "transport::http::mse",
        HttpMseNode,
        HttpMseConfig,
        ["transport", "http", "mse"],
        "Serves WebM or fragmented-MP4 (fMP4) streams to HTTP clients for MSE (Media \
         Source Extensions) playback. Accepts binary data from an upstream WebM or MP4 \
         muxer (container format auto-detected) and broadcasts to multiple concurrent \
         HTTP clients with init segment replay for late-joiners.",
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_find_cluster_id() {
        // No cluster
        assert_eq!(find_cluster_id(b"hello world"), None);

        // Cluster at start
        let data = vec![0x1F, 0x43, 0xB6, 0x75, 0x00, 0x01];
        assert_eq!(find_cluster_id(&data), Some(0));

        // Cluster after init segment
        let mut with_header = vec![0x1A, 0x45, 0xDF, 0xA3]; // EBML header ID
        with_header.extend_from_slice(&[0x00; 20]); // some header data
        with_header.extend_from_slice(&WEBM_CLUSTER_ID);
        with_header.extend_from_slice(&[0x00; 10]); // cluster data
        assert_eq!(find_cluster_id(&with_header), Some(24));

        // Too short
        assert_eq!(find_cluster_id(&[0x1F, 0x43]), None);
        assert_eq!(find_cluster_id(&[]), None);
    }

    #[test]
    fn test_http_mse_node_structure() {
        let config =
            HttpMseConfig { path: "/video".to_string(), max_clients: 10, content_type: None };
        let node = Box::new(HttpMseNode { config });

        // Verify pins
        assert_eq!(node.input_pins().len(), 1);
        assert_eq!(node.input_pins()[0].name, "in");
        assert_eq!(node.input_pins()[0].accepts_types, vec![PacketType::Binary]);
        assert_eq!(node.output_pins().len(), 0);
    }

    #[test]
    fn test_factory_rejects_zero_max_clients() {
        let factory = HttpMseNode::factory();
        let params = Some(serde_json::json!({
            "path": "/video",
            "max_clients": 0,
        }));
        let result = (factory)(params.as_ref());
        assert!(result.is_err());
    }

    #[test]
    fn test_factory_accepts_valid_config() {
        let factory = HttpMseNode::factory();
        let params = Some(serde_json::json!({
            "path": "/video",
            "max_clients": 5,
        }));
        let result = (factory)(params.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn test_factory_default_for_pin_inspection() {
        let factory = HttpMseNode::factory();
        let result = (factory)(None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_default_content_type_includes_codecs() {
        assert!(DEFAULT_CONTENT_TYPE.contains("codecs"));
    }

    /// Simulate the init-segment accumulation loop used by `HttpMseNode::run`
    /// so we can unit-test cross-chunk Cluster ID detection without spinning
    /// up a full async node context.
    fn simulate_init_accumulation(chunks: &[&[u8]]) -> (Vec<u8>, bool) {
        let mut init_segment: Vec<u8> = Vec::new();
        let mut init_complete = false;
        let mut overlap: Vec<u8> = Vec::new();
        let mut overlap_bytes_in_init: usize = 0;

        for data in chunks {
            if data.is_empty() {
                continue;
            }
            if init_complete {
                break;
            }

            let cluster_found = if overlap.is_empty() {
                false
            } else {
                let mut combined = overlap.clone();
                combined.extend_from_slice(&data[..data.len().min(WEBM_CLUSTER_ID.len())]);
                find_cluster_id(&combined).is_some_and(|pos| {
                    let overlap_len = overlap.len();
                    if pos < overlap_len {
                        let bytes_to_remove = (overlap_len - pos).min(overlap_bytes_in_init);
                        init_segment.truncate(init_segment.len() - bytes_to_remove);
                    } else {
                        let extra = pos - overlap_len;
                        let remaining_capacity =
                            MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                        let to_append = extra.min(remaining_capacity);
                        init_segment.extend_from_slice(&data[..to_append]);
                    }
                    true
                })
            };

            if cluster_found {
                init_complete = true;
            } else if let Some(cluster_offset) = find_cluster_id(data) {
                if cluster_offset > 0 {
                    let remaining_capacity =
                        MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                    let to_append = cluster_offset.min(remaining_capacity);
                    init_segment.extend_from_slice(&data[..to_append]);
                }
                init_complete = true;
            } else {
                let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                let appended = if remaining_capacity > 0 {
                    let to_append = data.len().min(remaining_capacity);
                    init_segment.extend_from_slice(&data[..to_append]);
                    to_append
                } else {
                    0
                };
                let keep = data.len().min(WEBM_CLUSTER_ID.len() - 1);
                overlap.clear();
                overlap.extend_from_slice(&data[data.len() - keep..]);
                overlap_bytes_in_init = appended.saturating_sub(data.len() - keep);
            }

            if init_complete {
                overlap.clear();
            }
        }

        (init_segment, init_complete)
    }

    /// Like `simulate_init_accumulation`, but also returns the forward data
    /// (bytes from the Cluster boundary onward) for the chunk that triggered
    /// init completion.  This lets us verify that cross-chunk Cluster ID
    /// straddling produces a complete Cluster header in the forwarded data.
    fn simulate_init_and_forward(chunks: &[&[u8]]) -> (Vec<u8>, Option<Vec<u8>>) {
        let mut init_segment: Vec<u8> = Vec::new();
        let mut init_complete = false;
        let mut overlap: Vec<u8> = Vec::new();
        let mut overlap_bytes_in_init: usize = 0;
        let mut forward_data: Option<Vec<u8>> = None;

        for data in chunks {
            if data.is_empty() {
                continue;
            }
            if init_complete {
                break;
            }

            let mut cluster_start_in_data: Option<usize> = None;
            let mut cluster_prefix: Vec<u8> = Vec::new();

            let cluster_found = if overlap.is_empty() {
                false
            } else {
                let mut combined = overlap.clone();
                combined.extend_from_slice(&data[..data.len().min(WEBM_CLUSTER_ID.len())]);
                find_cluster_id(&combined).is_some_and(|pos| {
                    let overlap_len = overlap.len();
                    if pos < overlap_len {
                        let bytes_to_remove = (overlap_len - pos).min(overlap_bytes_in_init);
                        init_segment.truncate(init_segment.len() - bytes_to_remove);
                        cluster_prefix = overlap[pos..].to_vec();
                        cluster_start_in_data = Some(0);
                    } else {
                        let extra = pos - overlap_len;
                        let remaining_capacity =
                            MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                        let to_append = extra.min(remaining_capacity);
                        init_segment.extend_from_slice(&data[..to_append]);
                        cluster_start_in_data = Some(extra);
                    }
                    true
                })
            };

            if cluster_found {
                init_complete = true;
            } else if let Some(cluster_offset) = find_cluster_id(data) {
                if cluster_offset > 0 {
                    let remaining_capacity =
                        MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                    let to_append = cluster_offset.min(remaining_capacity);
                    init_segment.extend_from_slice(&data[..to_append]);
                }
                init_complete = true;
                cluster_start_in_data = Some(cluster_offset);
            } else {
                let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                let appended = if remaining_capacity > 0 {
                    let to_append = data.len().min(remaining_capacity);
                    init_segment.extend_from_slice(&data[..to_append]);
                    to_append
                } else {
                    0
                };
                let keep = data.len().min(WEBM_CLUSTER_ID.len() - 1);
                overlap.clear();
                overlap.extend_from_slice(&data[data.len() - keep..]);
                overlap_bytes_in_init = appended.saturating_sub(data.len() - keep);
            }

            if init_complete {
                overlap.clear();
                // Build forward data the same way the production code does.
                if let Some(offset) = cluster_start_in_data {
                    if offset < data.len() {
                        if cluster_prefix.is_empty() {
                            forward_data = Some(data[offset..].to_vec());
                        } else {
                            let mut combined = cluster_prefix;
                            combined.extend_from_slice(&data[offset..]);
                            forward_data = Some(combined);
                        }
                    }
                }
            }
        }

        (init_segment, forward_data)
    }

    #[test]
    fn test_cross_chunk_cluster_detection_3_1_split() {
        // Cluster ID 0x1F43B675 split: first chunk ends with [1F, 43, B6],
        // second chunk starts with [75, ...].
        let header = vec![0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00]; // 6 bytes of EBML header
        let mut chunk1 = header.clone();
        chunk1.extend_from_slice(&[0x1F, 0x43, 0xB6]); // first 3 bytes of Cluster ID
        let chunk2: Vec<u8> = vec![0x75, 0xAA, 0xBB, 0xCC]; // last byte of Cluster ID + data

        let (init, complete) = simulate_init_accumulation(&[&chunk1, &chunk2]);

        assert!(complete, "Cluster ID straddling two chunks must be detected");
        // Init segment should contain only the header bytes (before the Cluster ID).
        assert_eq!(init, header, "Init segment should exclude the partial Cluster ID bytes");
    }

    #[test]
    fn test_cross_chunk_cluster_detection_2_2_split() {
        // Cluster ID split evenly: [1F, 43] | [B6, 75]
        let header = vec![0xAA; 10]; // 10 bytes of fake header
        let mut chunk1 = header.clone();
        chunk1.extend_from_slice(&[0x1F, 0x43]); // first 2 bytes of Cluster ID
        let chunk2: Vec<u8> = vec![0xB6, 0x75, 0x00, 0x01]; // last 2 bytes + data

        let (init, complete) = simulate_init_accumulation(&[&chunk1, &chunk2]);

        assert!(complete, "Cluster ID split 2|2 must be detected");
        assert_eq!(init, header, "Init segment should be only the header");
    }

    #[test]
    fn test_cross_chunk_cluster_detection_1_3_split() {
        // Cluster ID split: [1F] | [43, B6, 75]
        let header = vec![0xBB; 5]; // 5 bytes of fake header
        let mut chunk1 = header.clone();
        chunk1.push(0x1F); // first byte of Cluster ID
        let chunk2: Vec<u8> = vec![0x43, 0xB6, 0x75, 0xFF]; // remaining 3 bytes + data

        let (init, complete) = simulate_init_accumulation(&[&chunk1, &chunk2]);

        assert!(complete, "Cluster ID split 1|3 must be detected");
        assert_eq!(init, header, "Init segment should be only the header");
    }

    #[test]
    fn test_cluster_id_entirely_in_one_chunk() {
        // Cluster ID not straddling — entirely within chunk 2.
        let chunk1: Vec<u8> = vec![0x1A, 0x45, 0xDF, 0xA3, 0x00];
        let mut chunk2 = vec![0x00; 4]; // some data
        chunk2.extend_from_slice(&WEBM_CLUSTER_ID);
        chunk2.extend_from_slice(&[0xFF; 4]); // cluster data

        let (init, complete) = simulate_init_accumulation(&[&chunk1, &chunk2]);

        assert!(complete, "Cluster ID within a single chunk must be detected");
        // Init should be chunk1 + the 4 bytes before the Cluster ID in chunk2
        let mut expected = chunk1;
        expected.extend_from_slice(&[0x00; 4]);
        assert_eq!(init, expected);
    }

    #[test]
    fn test_empty_packets_skipped_during_init() {
        // Empty packets between real chunks should not break accumulation.
        let chunk1: Vec<u8> = vec![0x1A, 0x45, 0xDF, 0xA3];
        let empty: Vec<u8> = vec![];
        let mut chunk2 = vec![0x00; 2];
        chunk2.extend_from_slice(&WEBM_CLUSTER_ID);

        let (init, complete) = simulate_init_accumulation(&[&chunk1, &empty, &empty, &chunk2]);

        assert!(complete, "Init should complete despite empty packets");
        let mut expected = chunk1;
        expected.extend_from_slice(&[0x00; 2]);
        assert_eq!(init, expected);
    }

    #[test]
    fn test_cross_chunk_forward_data_includes_full_cluster_header_3_1() {
        // Cluster ID 0x1F43B675 split 3|1: overlap has [1F,43,B6], data starts with [75,...].
        // Forward data must start with the full Cluster ID, not just the tail byte.
        let header = vec![0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00];
        let mut chunk1 = header.clone();
        chunk1.extend_from_slice(&[0x1F, 0x43, 0xB6]); // first 3 bytes of Cluster ID
        let chunk2: Vec<u8> = vec![0x75, 0xAA, 0xBB, 0xCC]; // last byte + payload

        let (init, fwd) = simulate_init_and_forward(&[&chunk1, &chunk2]);
        assert_eq!(init, header);
        let fwd = fwd.expect("forward data should be present");
        assert!(
            fwd.starts_with(&WEBM_CLUSTER_ID),
            "forwarded data must start with the full Cluster ID, got {:02X?}",
            &fwd[..fwd.len().min(8)]
        );
    }

    #[test]
    fn test_cross_chunk_forward_data_includes_full_cluster_header_2_2() {
        // Cluster ID split 2|2: overlap has [1F,43], data starts with [B6,75,...].
        let header = vec![0xAA; 10];
        let mut chunk1 = header.clone();
        chunk1.extend_from_slice(&[0x1F, 0x43]); // first 2 bytes
        let chunk2: Vec<u8> = vec![0xB6, 0x75, 0xDD, 0xEE]; // last 2 bytes + payload

        let (init, fwd) = simulate_init_and_forward(&[&chunk1, &chunk2]);
        assert_eq!(init, header);
        let fwd = fwd.expect("forward data should be present");
        assert!(
            fwd.starts_with(&WEBM_CLUSTER_ID),
            "forwarded data must start with the full Cluster ID, got {:02X?}",
            &fwd[..fwd.len().min(8)]
        );
    }

    #[test]
    fn test_non_straddling_forward_data_starts_at_cluster() {
        // Cluster ID entirely within one chunk — forward data should start at the Cluster ID.
        let header: Vec<u8> = vec![0x1A, 0x45, 0xDF, 0xA3, 0x00];
        let mut chunk = header.clone();
        chunk.extend_from_slice(&WEBM_CLUSTER_ID);
        chunk.extend_from_slice(&[0xFF; 4]);

        let (init, fwd) = simulate_init_and_forward(&[&chunk]);
        assert_eq!(init, header);
        let fwd = fwd.expect("forward data should be present");
        assert!(
            fwd.starts_with(&WEBM_CLUSTER_ID),
            "forwarded data must start with the Cluster ID, got {:02X?}",
            &fwd[..fwd.len().min(8)]
        );
    }

    #[test]
    fn test_find_tracks_end_basic() {
        // Tracks ID (4 bytes) + VINT size 0x2E=46 (1 byte) + 46 bytes content
        let mut data = Vec::new();
        data.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3]); // EBML header ID
        data.extend_from_slice(&[0x00; 10]); // some header bytes
        data.extend_from_slice(&WEBM_TRACKS_ID); // Tracks element ID
        data.push(0x85); // VINT: 1-byte size = 5 (0x85 & 0x7F = 5)
        data.extend_from_slice(&[0xAA; 5]); // 5 bytes of Tracks content

        // Tracks element ends at: 14 (start) + 4 (ID) + 1 (size) + 5 (content) = 24
        assert_eq!(find_tracks_end(&data), Some(24));
    }

    #[test]
    fn test_find_tracks_end_2_byte_vint() {
        // 2-byte VINT size: 0x40 | high_byte, low_byte
        // 0x41 0x00 → width=2, value = (0x41 & 0x3F)<<8 | 0x00 = 0x0100 = 256
        let mut data = Vec::new();
        data.extend_from_slice(&WEBM_TRACKS_ID);
        data.extend_from_slice(&[0x41, 0x00]); // 2-byte VINT = 256
        data.extend_from_slice(&[0xBB; 256]); // 256 bytes of content

        assert_eq!(find_tracks_end(&data), Some(4 + 2 + 256));
    }

    #[test]
    fn test_find_tracks_end_not_found() {
        let data = vec![0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00];
        assert_eq!(find_tracks_end(&data), None);
    }

    #[test]
    fn test_find_tracks_end_truncated_size() {
        // Tracks ID present but size VINT is truncated (data ends too early).
        let mut data = Vec::new();
        data.extend_from_slice(&WEBM_TRACKS_ID);
        // No size byte follows — data is truncated.
        assert_eq!(find_tracks_end(&data), None);
    }

    #[test]
    fn test_find_tracks_end_truncated_content() {
        // Tracks ID + size says 100 bytes, but only 10 bytes of content.
        let mut data = Vec::new();
        data.extend_from_slice(&WEBM_TRACKS_ID);
        data.push(0x80 | 0x64); // 1-byte VINT = 100
        data.extend_from_slice(&[0xCC; 10]); // only 10 bytes

        // find_tracks_end returns the *logical* end (ID pos + 4 + 1 + 100 = 105)
        // even if the data is shorter.  The caller should check bounds.
        assert_eq!(find_tracks_end(&data), Some(4 + 1 + 100));
    }

    #[test]
    fn test_find_tracks_end_8_byte_vint() {
        // 8-byte VINT: first byte = 0x01 (width=8), remaining 7 bytes encode size.
        // 0x01 0x00 0x00 0x00 0x00 0x00 0x00 0x0A → size = 10
        let mut data = Vec::new();
        data.extend_from_slice(&WEBM_TRACKS_ID);
        data.extend_from_slice(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0A]); // 8-byte VINT = 10
        data.extend_from_slice(&[0xEE; 10]); // 10 bytes of content

        // Tracks end: 4 (ID) + 8 (VINT) + 10 (content) = 22
        assert_eq!(find_tracks_end(&data), Some(22));
    }

    #[test]
    fn test_find_cluster_preamble_end_basic() {
        // Cluster ID (4) + unknown size VINT 8-byte (8) + Timecode 0xE7 (1) + VINT size 0x82 (1) + 2 bytes value
        // Total preamble: 4 + 8 + 1 + 1 + 2 = 16
        let mut data = Vec::new();
        data.extend_from_slice(&WEBM_CLUSTER_ID);
        data.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]); // unknown size
        data.push(WEBM_TIMECODE_ID); // 0xE7
        data.push(0x82); // VINT size = 2
        data.extend_from_slice(&[0x0F, 0x9F]); // 2 bytes timecode value
        data.push(0xA3); // SimpleBlock after preamble

        assert_eq!(find_cluster_preamble_end(&data), Some(16));
    }

    #[test]
    fn test_find_cluster_preamble_end_no_timecode() {
        // Cluster ID (4) + unknown size (8), then immediately a SimpleBlock (not Timecode).
        let mut data = Vec::new();
        data.extend_from_slice(&WEBM_CLUSTER_ID);
        data.extend_from_slice(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        data.push(0xA3); // SimpleBlock instead of Timecode

        // Should return 12 (just past the size VINT, since there's no Timecode).
        assert_eq!(find_cluster_preamble_end(&data), Some(12));
    }

    #[test]
    fn test_find_cluster_preamble_end_truncated() {
        // Only the Cluster ID, no size VINT.
        assert_eq!(find_cluster_preamble_end(&WEBM_CLUSTER_ID), None);
    }

    #[test]
    fn test_init_segment_truncated_at_tracks_end() {
        // Simulate a WebM stream where the muxer emits SimpleBlock data
        // (element ID 0xA3) between the Tracks element and the first Cluster.
        // The init segment must be truncated to exclude that invalid data.

        // Build: EBML(4) + Info(6) + Tracks(4+1+8=13) + garbage(20) + Cluster
        let mut header = Vec::new();
        header.extend_from_slice(&[0x1A, 0x45, 0xDF, 0xA3]); // EBML ID
        header.extend_from_slice(&[0x00; 6]); // Info placeholder
        header.extend_from_slice(&WEBM_TRACKS_ID); // Tracks ID
        header.push(0x88); // VINT: 1-byte size = 8
        header.extend_from_slice(&[0xDD; 8]); // 8 bytes Tracks content

        let tracks_end = header.len(); // 4 + 6 + 4 + 1 + 8 = 23

        // Add invalid SimpleBlock data between Tracks and Cluster.
        let mut stream = header.clone();
        stream.extend_from_slice(&[0xA3; 20]); // fake SimpleBlock bytes
        stream.extend_from_slice(&WEBM_CLUSTER_ID);
        stream.extend_from_slice(&[0xFF; 10]); // cluster data

        let (init, complete) = simulate_init_accumulation(&[&stream]);

        assert!(complete, "Cluster must be detected");
        // The raw accumulated init (before truncation) includes the garbage.
        // But simulate_init_accumulation doesn't call find_tracks_end —
        // that happens in the node's run loop.  Verify find_tracks_end works:
        assert_eq!(find_tracks_end(&init), Some(tracks_end));

        // Now simulate what the node does: truncate at tracks_end.
        let mut truncated = init;
        if let Some(end) = find_tracks_end(&truncated) {
            truncated.truncate(end);
        }
        assert_eq!(truncated.len(), tracks_end);
        assert_eq!(truncated, header);
    }

    /// Build a top-level ISO-BMFF box: `[size: u32 BE][type][body]`.
    fn mp4_box(box_type: [u8; 4], body: &[u8]) -> Vec<u8> {
        let size = u32::try_from(8 + body.len()).expect("test box fits in u32");
        let mut out = size.to_be_bytes().to_vec();
        out.extend_from_slice(&box_type);
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn test_detect_mse_format() {
        assert_eq!(detect_mse_format(&[0x1A, 0x45, 0xDF, 0xA3, 0x00]), Some(MseFormat::WebM));

        let ftyp = mp4_box(*b"ftyp", b"isom");
        assert_eq!(detect_mse_format(&ftyp), Some(MseFormat::Fmp4));

        assert_eq!(detect_mse_format(b"random bytes here"), None);
        assert_eq!(detect_mse_format(&[0x00, 0x01]), None);
        assert_eq!(detect_mse_format(&[]), None);
    }

    #[test]
    fn test_format_from_content_type() {
        assert_eq!(
            format_from_content_type("video/mp4; codecs=\"avc1.42c01f\""),
            Some(MseFormat::Fmp4)
        );
        assert_eq!(
            format_from_content_type("video/webm; codecs=\"vp9,opus\""),
            Some(MseFormat::WebM)
        );
        assert_eq!(format_from_content_type("VIDEO/MP4"), Some(MseFormat::Fmp4));
        assert_eq!(format_from_content_type("video/x-matroska"), Some(MseFormat::WebM));
        assert_eq!(format_from_content_type("application/octet-stream"), None);
        assert_eq!(format_from_content_type(""), None);
    }

    #[test]
    fn test_fmp4_find_first_moof_after_init() {
        let ftyp = mp4_box(*b"ftyp", b"isom\x00\x00\x02\x00");
        let moov = mp4_box(*b"moov", &[0xAB; 64]);
        let fragment = mp4_box(*b"moof", &[0xCD; 16]);

        let mut stream = ftyp.clone();
        stream.extend_from_slice(&moov);
        stream.extend_from_slice(&fragment);
        stream.extend_from_slice(&mp4_box(*b"mdat", &[0xEE; 32]));

        assert_eq!(fmp4_find_first_moof(&stream), Some(ftyp.len() + moov.len()));
    }

    #[test]
    fn test_fmp4_find_first_moof_at_start() {
        let fragment = mp4_box(*b"moof", &[0x00; 8]);
        assert_eq!(fmp4_find_first_moof(&fragment), Some(0));
    }

    #[test]
    fn test_fmp4_find_first_moof_none_without_moof() {
        let mut stream = mp4_box(*b"ftyp", b"isom");
        stream.extend_from_slice(&mp4_box(*b"moov", &[0x11; 20]));
        assert_eq!(fmp4_find_first_moof(&stream), None);
    }

    #[test]
    fn test_fmp4_find_first_moof_waits_for_incomplete_moov() {
        // moov declares 64 body bytes but only 10 are buffered, so the parser
        // cannot yet see whether a moof follows.
        let ftyp = mp4_box(*b"ftyp", b"isom");
        let mut moov_header = ((8 + 64u32).to_be_bytes()).to_vec();
        moov_header.extend_from_slice(b"moov");
        moov_header.extend_from_slice(&[0x22; 10]); // partial body

        let mut stream = ftyp;
        stream.extend_from_slice(&moov_header);
        assert_eq!(fmp4_find_first_moof(&stream), None);
    }

    #[test]
    fn test_fmp4_find_first_moof_ignores_moof_bytes_in_mdat() {
        // The literal bytes b"moof" inside an mdat payload must not be mistaken
        // for a box header — box-length walking skips the whole mdat body.
        let ftyp = mp4_box(*b"ftyp", b"isom");
        let moov = mp4_box(*b"moov", &[0x33; 16]);
        let mut mdat_body = vec![0x00; 4];
        mdat_body.extend_from_slice(b"moof"); // decoy inside payload
        mdat_body.extend_from_slice(&[0x00; 8]);
        let mdat = mp4_box(*b"mdat", &mdat_body);

        let mut stream = ftyp;
        stream.extend_from_slice(&moov);
        stream.extend_from_slice(&mdat);

        // No real moof box exists, despite the decoy bytes in mdat.
        assert_eq!(fmp4_find_first_moof(&stream), None);
    }

    #[test]
    fn test_fmp4_find_first_moof_skips_largesize_box() {
        // A box using the 64-bit `largesize` form (size32 == 1) before moof.
        let mut big = vec![0x00, 0x00, 0x00, 0x01]; // size32 = 1 → largesize follows
        big.extend_from_slice(b"free");
        let body_len = 24u64;
        big.extend_from_slice(&(16 + body_len).to_be_bytes()); // largesize
        big.extend_from_slice(&[0x44; 24]);

        let fragment = mp4_box(*b"moof", &[0x00; 8]);
        let mut stream = big.clone();
        stream.extend_from_slice(&fragment);

        assert_eq!(fmp4_find_first_moof(&stream), Some(big.len()));
    }

    /// Simulate the fMP4 init-segment branch of `HttpMseNode::run`: accumulate
    /// chunks until the first `moof`, returning the init segment (ftyp+moov)
    /// and the forwarded media bytes for the chunk that completed init.
    ///
    /// Mirrors the real loop: the whole packet is appended and searched before
    /// any size guard (so an oversized first packet is never truncated), and
    /// once the buffer reaches the cap without a moof it is frozen rather than
    /// truncated (truncate-then-append would corrupt the box chain).
    fn simulate_fmp4_init_and_forward(chunks: &[&[u8]]) -> (Vec<u8>, Option<Vec<u8>>) {
        let mut init_segment: Vec<u8> = Vec::new();
        let mut init_complete = false;
        let mut forward_data: Option<Vec<u8>> = None;

        for data in chunks {
            if data.is_empty() || init_complete {
                continue;
            }
            if init_segment.len() < MAX_FMP4_INIT_SEGMENT_SIZE {
                init_segment.extend_from_slice(data);
            }
            let Some(moof_off) = fmp4_find_first_moof(&init_segment) else {
                continue;
            };
            forward_data = Some(init_segment[moof_off..].to_vec());
            init_segment.truncate(moof_off);
            init_complete = true;
        }

        (init_segment, forward_data)
    }

    #[test]
    fn test_fmp4_init_single_chunk() {
        let ftyp = mp4_box(*b"ftyp", b"isom\x00\x00\x02\x00");
        let moov = mp4_box(*b"moov", &[0xAB; 40]);
        let fragment = mp4_box(*b"moof", &[0xCD; 12]);
        let mdat = mp4_box(*b"mdat", &[0xEE; 24]);

        let mut init = ftyp;
        init.extend_from_slice(&moov);
        let mut media = fragment;
        media.extend_from_slice(&mdat);
        let mut stream = init.clone();
        stream.extend_from_slice(&media);

        let (init_seg, fwd) = simulate_fmp4_init_and_forward(&[&stream]);
        assert_eq!(init_seg, init, "init segment must be exactly ftyp+moov");
        let fwd = fwd.expect("forward data should be present");
        assert_eq!(fwd, media, "forwarded media must start at the moof box");
        assert_eq!(fmp4_find_first_moof(&fwd), Some(0));
    }

    #[test]
    fn test_fmp4_init_split_across_chunks() {
        let ftyp = mp4_box(*b"ftyp", b"isom");
        let moov = mp4_box(*b"moov", &[0x55; 48]);
        let fragment = mp4_box(*b"moof", &[0x66; 16]);

        let split = ftyp.len() + 12;
        let mut init = ftyp;
        init.extend_from_slice(&moov);

        // Split the init segment mid-moov, with moof arriving in a later chunk.
        let chunk1 = &init[..split];
        let chunk2 = &init[split..];

        let (init_seg, fwd) = simulate_fmp4_init_and_forward(&[chunk1, chunk2, &fragment]);
        assert_eq!(init_seg, init);
        assert_eq!(fwd.expect("forward present"), fragment);
    }

    #[test]
    fn test_fmp4_init_oversized_first_packet_not_truncated() {
        // The muxer emits ftyp+moov AND the first moof+mdat as one packet that
        // can exceed MAX_FMP4_INIT_SEGMENT_SIZE. The media segment must survive
        // intact — the memory guard only applies before a moof is found.
        let ftyp = mp4_box(*b"ftyp", b"isom");
        let moov = mp4_box(*b"moov", &[0x77; 256]);
        let fragment = mp4_box(*b"moof", &[0x88; 32]);
        let mdat = mp4_box(*b"mdat", &vec![0x99; MAX_FMP4_INIT_SEGMENT_SIZE]);

        let mut init = ftyp;
        init.extend_from_slice(&moov);
        let mut media = fragment;
        media.extend_from_slice(&mdat);
        let mut stream = init.clone();
        stream.extend_from_slice(&media);
        assert!(stream.len() > MAX_FMP4_INIT_SEGMENT_SIZE);

        let (init_seg, fwd) = simulate_fmp4_init_and_forward(&[&stream]);
        assert_eq!(init_seg, init, "init segment must be exactly ftyp+moov");
        assert_eq!(
            fwd.expect("forward data should be present"),
            media,
            "the full first media segment must be forwarded uncorrupted"
        );
    }

    #[test]
    fn test_fmp4_init_moov_exceeds_cap_freezes_without_corruption() {
        // Pathological: ftyp+moov ALONE exceeds the cap before any moof arrives,
        // delivered across many chunks. The buffer must freeze at the cap (never
        // truncate-then-append, which would leave a gap the box-walker can never
        // re-align past) so init never completes — but memory stays bounded and
        // nothing is forwarded.
        let ftyp = mp4_box(*b"ftyp", b"isom");
        let moov = mp4_box(*b"moov", &vec![0x55; MAX_FMP4_INIT_SEGMENT_SIZE]);
        let fragment = mp4_box(*b"moof", &[0x88; 32]);
        let mdat = mp4_box(*b"mdat", &[0x99; 64]);

        let mut stream = ftyp;
        stream.extend_from_slice(&moov);
        stream.extend_from_slice(&fragment);
        stream.extend_from_slice(&mdat);

        let chunk_size = 64 * 1024;
        let chunks: Vec<&[u8]> = stream.chunks(chunk_size).collect();
        let (init_seg, fwd) = simulate_fmp4_init_and_forward(&chunks);

        assert!(fwd.is_none(), "no media should be forwarded when init never completes");
        assert!(
            init_seg.len() <= MAX_FMP4_INIT_SEGMENT_SIZE + chunk_size,
            "frozen init buffer must stay bounded (was {})",
            init_seg.len()
        );
        assert_eq!(
            init_seg,
            stream[..init_seg.len()],
            "buffer must be an uncorrupted prefix of the stream (no truncate-then-append gap)"
        );
    }
}
