// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! HTTP MSE output node — serves WebM streams to HTTP clients for MSE playback.
//!
//! This node accepts `Packet::Binary` data (WebM chunks) from an upstream muxer
//! and broadcasts them to connected HTTP clients as chunked responses. Late-joining
//! clients receive the buffered WebM initialization segment so MSE
//! `SourceBuffer.appendBuffer()` works correctly.

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

/// Configuration for the HttpMse node.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

/// A node that serves WebM binary data to HTTP clients for MSE playback.
///
/// It accepts `Packet::Binary` from an upstream WebM muxer (in Live mode) and
/// fans out the chunks to all connected HTTP clients. The WebM initialization
/// segment (EBML header + Segment + Tracks) is buffered and replayed to
/// late-joining clients.
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

        // Get the MSE gateway from the global registry.
        let gateway = streamkit_core::mse_gateway::get_mse_gateway().ok_or_else(|| {
            let err = "MSE gateway not available — ensure transport::http::mse is used in a session with gateway support";
            tracing::error!("{}", err);
            StreamKitError::Runtime(err.to_string())
        })?;

        let content_type =
            self.config.content_type.clone().unwrap_or_else(|| DEFAULT_CONTENT_TYPE.to_string());

        // Build the full registration path: /mse/{session_id}{path}
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

        // Register with the MSE gateway, passing max_clients so the gateway
        // can enforce capacity and reject clients with a proper 503.
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

        // Take ownership of the input channel.
        let mut input_rx = context.take_input("in").map_err(|e| {
            tracing::error!("Failed to take input pin: {}", e);
            e
        })?;

        state_helpers::emit_running(&context.state_tx, &node_name);
        let node_start = std::time::Instant::now();

        let mut stats_tracker = NodeStatsTracker::new(node_name.clone(), context.stats_tx.clone());

        // Connected clients (body_tx senders).
        let mut clients: Vec<mpsc::Sender<Bytes>> = Vec::new();

        // Init segment buffer: accumulates bytes until the first WebM Cluster is
        // detected, then truncated to only the header (EBML + Segment + Info +
        // Tracks).  Any SimpleBlock data the muxer writes before opening the
        // first Cluster is discarded — it is invalid for MSE SourceBuffer.
        //
        // Per the WebM Byte Stream Format spec for MSE, the init segment must
        // end before the first Cluster element.
        //
        // Rolling GOP buffer: accumulates data from the most recent Cluster
        // start (keyframe boundary).  Late-joining clients receive init +
        // this buffer so they start at a clean Cluster with a keyframe.
        // The buffer is reset whenever a new Cluster ID is detected in the
        // live data.  A hard cap prevents unbounded growth if keyframes are
        // infrequent (e.g. 10s interval at high bitrate).
        const MAX_GOP_BUFFER_SIZE: usize = 2 * 1024 * 1024; // 2 MB
        let mut init_segment: Vec<u8> = Vec::new();
        let mut gop_buffer: Vec<u8> = Vec::new();
        let mut init_complete = false;

        // Overlap buffer for detecting Cluster ID across chunk boundaries.
        // Holds the last 3 bytes of the previous chunk so a 4-byte Cluster ID
        // that straddles two consecutive packets can still be found.
        let mut overlap: Vec<u8> = Vec::new();
        // How many of the overlap bytes were actually appended to init_segment.
        // When the buffer is near MAX_INIT_SEGMENT_SIZE, the overlap tail may
        // extend past what was buffered, so truncation must be bounded by this.
        let mut overlap_bytes_in_init: usize = 0;

        let cancellation_token = context.cancellation_token.clone();

        let result: Result<(), StreamKitError> = loop {
            tokio::select! {
                biased;

                // Shutdown via cancellation token.
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

                // Control messages.
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

                // New HTTP client connecting.
                client = client_rx.recv() => {
                    let Some(client) = client else {
                        tracing::warn!("MSE gateway client channel closed");
                        break Ok(());
                    };

                    // Send init segment + rolling GOP buffer to the new client.
                    // The GOP buffer starts at the most recent Cluster boundary
                    // (keyframe), so the client can decode immediately.
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

                // Incoming WebM data from upstream muxer.
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

                    // Skip empty packets — they carry no data and would
                    // pass through init-detection doing nothing useful.
                    if data.is_empty() {
                        continue;
                    }

                    // Buffer the init segment (everything before the first Cluster element).
                    // When the first Cluster is found, `cluster_start_in_data` records
                    // the byte offset within `data` where the Cluster begins so only
                    // valid media data (from the Cluster onward) is forwarded to clients.
                    let mut cluster_start_in_data: Option<usize> = None;

                    if !init_complete {
                        // Search for the Cluster ID in the overlap + current chunk to handle
                        // the case where the 4-byte ID straddles two consecutive packets.
                        let cluster_found = if overlap.is_empty() {
                            false
                        } else {
                            let mut combined = overlap.clone();
                            combined.extend_from_slice(&data[..data.len().min(WEBM_CLUSTER_ID.len())]);
                            find_cluster_id(&combined).is_some_and(|pos| {
                                let overlap_len = overlap.len();
                                if pos < overlap_len {
                                    // The Cluster ID starts inside the overlap region.
                                    // Only truncate bytes that were actually appended to
                                    // init_segment — the overlap may extend past what was
                                    // buffered if the init_segment hit MAX_INIT_SEGMENT_SIZE.
                                    let bytes_to_remove = (overlap_len - pos).min(overlap_bytes_in_init);
                                    init_segment.truncate(init_segment.len() - bytes_to_remove);
                                    // Cluster ID straddles: the remaining bytes are at the
                                    // start of `data`.  Forward from byte 0.
                                    cluster_start_in_data = Some(0);
                                } else {
                                    // The Cluster started inside the new chunk.
                                    let extra = pos - overlap_len;
                                    let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                    let to_append = extra.min(remaining_capacity);
                                    init_segment.extend_from_slice(&data[..to_append]);
                                    // Forward data from the Cluster offset within this chunk.
                                    cluster_start_in_data = Some(extra);
                                }
                                true
                            })
                        };

                        if cluster_found {
                            init_complete = true;
                        } else if let Some(cluster_offset) = find_cluster_id(&data) {
                            // Cluster ID found entirely within this chunk.
                            if cluster_offset > 0 {
                                let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                                let to_append = cluster_offset.min(remaining_capacity);
                                init_segment.extend_from_slice(&data[..to_append]);
                            }
                            init_complete = true;
                            cluster_start_in_data = Some(cluster_offset);
                        } else {
                            // Still in the init segment — buffer these bytes.
                            let remaining_capacity = MAX_INIT_SEGMENT_SIZE.saturating_sub(init_segment.len());
                            let appended = if remaining_capacity > 0 {
                                let to_append = data.len().min(remaining_capacity);
                                init_segment.extend_from_slice(&data[..to_append]);
                                to_append
                            } else {
                                0
                            };
                            // Keep the last 3 bytes for cross-chunk Cluster ID detection.
                            let keep = data.len().min(WEBM_CLUSTER_ID.len() - 1);
                            overlap.clear();
                            overlap.extend_from_slice(&data[data.len() - keep..]);
                            // Track how many of those overlap bytes are actually in init_segment.
                            // The overlap comes from the tail of `data`, but only `appended` bytes
                            // from `data` were added to init_segment. The overlap bytes start at
                            // offset `data.len() - keep`, so they're in init_segment only if
                            // `appended > data.len() - keep`.
                            overlap_bytes_in_init = appended.saturating_sub(data.len() - keep);
                        }

                        if init_complete {
                            // Truncate the init segment to the WebM header
                            // (EBML + Segment + Info + Tracks).  The muxer may
                            // emit SimpleBlock data before the first Cluster in
                            // non-seekable streaming mode; those bytes are
                            // invalid at the Segment level and would cause
                            // MSE CHUNK_DEMUXER_ERROR_APPEND_FAILED.
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

                            // No Cluster preamble is stored — see comment above.

                            tracing::info!(
                                init_segment_size = init_segment.len(),
                                elapsed_ms = node_start.elapsed().as_millis() as u64,
                                "WebM init segment captured"
                            );
                            overlap.clear();

                            // Send init segment to clients that connected before
                            // init was complete.  They've been waiting in the
                            // `clients` vec without data.
                            if !clients.is_empty() && !init_segment.is_empty() {
                                let init_bytes = Bytes::copy_from_slice(&init_segment);
                                let mut i = 0;
                                while i < clients.len() {
                                    match clients[i].try_send(init_bytes.clone()) {
                                        Ok(()) => { i += 1; }
                                        Err(mpsc::error::TrySendError::Closed(_) | mpsc::error::TrySendError::Full(_)) => {
                                            clients.swap_remove(i);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Only forward data once the init segment is complete
                    // (i.e. the first Cluster has arrived).  Data emitted by
                    // the muxer before the first Cluster contains SimpleBlock
                    // elements at the Segment level which are invalid for MSE.
                    if !init_complete {
                        continue;
                    }

                    // For the chunk that triggered init_complete, forward data
                    // from the Cluster boundary onward (including the Cluster
                    // header).  Clients receive complete Clusters with their
                    // own headers so they can start decoding at any keyframe.
                    let forward_data = match cluster_start_in_data {
                        Some(offset) if offset < data.len() => data.slice(offset..),
                        Some(_) => continue,
                        None => data.clone(),
                    };

                    // Maintain the rolling GOP buffer for late-joining clients.
                    // Reset when we see a new Cluster ID; append otherwise.
                    if !forward_data.is_empty() {
                        if find_cluster_id(&forward_data).is_some() {
                            tracing::debug!(
                                gop_size = gop_buffer.len(),
                                elapsed_ms = node_start.elapsed().as_millis() as u64,
                                "HTTP MSE: new Cluster (GOP reset)"
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

                    // Broadcast to all connected clients, removing disconnected or slow ones.
                    if !clients.is_empty() && !forward_data.is_empty() {
                        let chunk = forward_data;
                        let mut i = 0;
                        while i < clients.len() {
                            // Use try_send to avoid blocking the pipeline on a slow client.
                            match clients[i].try_send(chunk.clone()) {
                                Ok(()) => {
                                    // stats_tracker.sent() counts per-client delivery, not per-packet.
                                    // This differs from single-output nodes but accurately reflects
                                    // the total number of chunk deliveries across all clients.
                                    stats_tracker.sent();
                                    i += 1;
                                }
                                Err(mpsc::error::TrySendError::Closed(_)) => {
                                    clients.swap_remove(i);
                                    tracing::debug!(
                                        client_count = clients.len(),
                                        "MSE client disconnected"
                                    );
                                }
                                Err(mpsc::error::TrySendError::Full(_)) => {
                                    // Dropping chunks from a WebM stream corrupts the container
                                    // format — MSE SourceBuffer will throw decode errors with
                                    // no recovery path. Disconnect the slow client entirely
                                    // rather than silently dropping data.
                                    clients.swap_remove(i);
                                    tracing::warn!(
                                        client_count = clients.len(),
                                        "MSE client too slow, disconnecting to avoid corrupt stream"
                                    );
                                }
                            }
                        }
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
#[cfg_attr(not(test), allow(dead_code))]
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
///
/// # Panics
///
/// Panics if the config schema cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)]
pub fn register_http_mse_nodes(registry: &mut streamkit_core::NodeRegistry) {
    use schemars::schema_for;

    let factory = HttpMseNode::factory();
    registry.register_dynamic_with_description(
        "transport::http::mse",
        move |params| (factory)(params),
        serde_json::to_value(schema_for!(HttpMseConfig))
            .expect("HttpMseConfig schema should serialize to JSON"),
        vec!["transport".to_string(), "http".to_string(), "mse".to_string()],
        false,
        "Serves WebM streams to HTTP clients for MSE (Media Source Extensions) playback. \
         Accepts binary data from an upstream WebM muxer and broadcasts to multiple \
         concurrent HTTP clients with init segment replay for late-joiners.",
    );
}

#[cfg(test)]
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

    // ── find_tracks_end tests ──

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
        data.push(0x80 | 100); // 1-byte VINT = 100
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

    // ── Cluster preamble parsing ──

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

    // ── Init segment truncation at Tracks end ──

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
}
