// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    Json,
};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::session::{system_time_to_rfc3339, PreviewState, TapPoint, MAX_PREVIEWS_PER_SESSION};
use crate::state::AppState;
use streamkit_api::Pipeline;
use streamkit_core::control::EngineControlMessage;
use streamkit_core::types::PacketType;

use super::read_registry;

// ── Request / response types ─────────────────────────────────────────

#[derive(Deserialize)]
pub struct StartPreviewRequest {
    pub tap_node: Option<String>,
    pub tap_pin: Option<String>,
}

#[derive(Serialize)]
pub struct PreviewResponse {
    pub preview_id: String,
    pub gateway_path: String,
    pub broadcast: String,
    pub audio: bool,
    pub video: bool,
}

#[derive(Serialize)]
pub struct PreviewInfo {
    pub preview_id: String,
    pub gateway_path: String,
    pub broadcast: String,
    pub audio: bool,
    pub video: bool,
    pub tap_node: String,
    pub tap_pin: String,
    pub tap_points: Vec<TapPointInfo>,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct TapPointInfo {
    pub node: String,
    pub pin: String,
    pub media: String,
}

// ── Terminal node kinds (sinks that don't produce useful output) ─────

const TERMINAL_KINDS: &[&str] =
    &["transport::moq::publisher", "transport::moq::peer", "core::sink", "io::file_writer"];

fn is_terminal_kind(kind: &str) -> bool {
    TERMINAL_KINDS.contains(&kind)
}

// ── Output pin classification ────────────────────────────────────────

/// Classify a node output pin as `(is_encoded, is_audio, is_video)`.
///
/// Uses the node registry to look up the pin's `produces_type`. Falls back
/// to a kind-based heuristic when the registry lookup fails (e.g. for
/// plugin or dynamic nodes).
fn classify_output_pin(
    node_kind: &str,
    pin_name: &str,
    registry: &streamkit_core::NodeRegistry,
) -> (bool, bool, bool) {
    if let Some(def) = registry.get_definition(node_kind) {
        for pin in &def.outputs {
            if pin.name == pin_name {
                return match &pin.produces_type {
                    PacketType::EncodedAudio(_) => (true, true, false),
                    PacketType::EncodedVideo(_) => (true, false, true),
                    PacketType::RawAudio(_) => (false, true, false),
                    PacketType::RawVideo(_) => (false, false, true),
                    // For Any/Passthrough/Binary, fall through to heuristic
                    _ => classify_by_kind(node_kind),
                };
            }
        }
    }
    classify_by_kind(node_kind)
}

fn classify_by_kind(kind: &str) -> (bool, bool, bool) {
    match kind {
        k if k.contains("vp9::encoder") || k.contains("h264::encoder") => (true, false, true),
        k if k.contains("opus::encoder") || k.contains("aac::encoder") => (true, true, false),
        k if k.contains("compositor") || k.contains("pixel_convert") => (false, false, true),
        k if k.contains("mixer") || k.contains("resampler") || k.contains("gain") => {
            (false, true, false)
        },
        _ => {
            tracing::debug!(kind = %kind, "classify_by_kind: unknown node kind, skipping");
            (false, false, false)
        },
    }
}

// ── Auto-detect tap points ───────────────────────────────────────────

/// Find the best tap points by tracing connections into terminal nodes.
///
/// Returns one tap point per media type (audio and/or video). For
/// pipelines with separate audio and video encoder chains feeding the
/// same terminal node, this returns both. Prefers tapping after
/// encoders (encoded output) to skip re-encoding.
///
/// # Errors
///
/// Returns an error if the pipeline has no suitable output nodes to tap.
pub fn detect_tap_points(
    pipeline: &Pipeline,
    registry: &streamkit_core::NodeRegistry,
) -> Result<Vec<TapPoint>, String> {
    // Gather candidate tap points: connections feeding into terminal nodes.
    let mut candidates: Vec<TapPoint> = Vec::new();

    for conn in &pipeline.connections {
        let Some(target_node) = pipeline.nodes.get(&conn.to_node) else {
            continue;
        };
        if !is_terminal_kind(&target_node.kind) {
            continue;
        }

        let Some(source_node) = pipeline.nodes.get(&conn.from_node) else {
            continue;
        };

        let (is_encoded, is_audio, is_video) =
            classify_output_pin(&source_node.kind, &conn.from_pin, registry);

        if !is_audio && !is_video {
            continue;
        }

        candidates.push(TapPoint {
            node: conn.from_node.clone(),
            pin: conn.from_pin.clone(),
            is_encoded,
            is_audio,
            is_video,
        });
    }

    if candidates.is_empty() {
        // Fallback: pick the first non-terminal node that has outgoing
        // connections (i.e. it produces output that something consumes).
        for conn in &pipeline.connections {
            let Some(source_node) = pipeline.nodes.get(&conn.from_node) else {
                continue;
            };
            if !is_terminal_kind(&source_node.kind) {
                return Ok(vec![TapPoint {
                    node: conn.from_node.clone(),
                    pin: conn.from_pin.clone(),
                    is_encoded: false,
                    is_audio: false,
                    is_video: true, // assume video for fallback
                }]);
            }
        }

        return Err(
            "Cannot auto-detect tap point: pipeline has no suitable output nodes".to_string()
        );
    }

    // Prefer encoded candidates (no re-encoding needed).
    let encoded: Vec<&TapPoint> = candidates.iter().filter(|c| c.is_encoded).collect();
    let chosen_refs: Vec<&TapPoint> =
        if encoded.is_empty() { candidates.iter().collect() } else { encoded };

    // Deduplicate by media type: pick at most one audio and one video tap.
    let mut result: Vec<TapPoint> = Vec::new();
    let mut has_audio = false;
    let mut has_video = false;

    for tp in chosen_refs {
        if tp.is_audio && !has_audio {
            result.push(tp.clone());
            has_audio = true;
        } else if tp.is_video && !has_video {
            result.push(tp.clone());
            has_video = true;
        }
        if has_audio && has_video {
            break;
        }
    }

    // If nothing matched the encoded/raw preference, just take what we have.
    if result.is_empty() {
        result.push(candidates[0].clone());
    }

    Ok(result)
}

// ── Subgraph injection ───────────────────────────────────────────────

/// Pre-compute the output pin classification while we still hold the
/// registry read-guard (which is `!Send` and must be dropped before the
/// next `.await`).
fn classify_output_pin_from_registry(
    pipeline: &Pipeline,
    tap_node: &str,
    tap_pin: &str,
    registry: &streamkit_core::NodeRegistry,
) -> (bool, bool, bool) {
    pipeline
        .nodes
        .get(tap_node)
        .map_or((false, false, false), |n| classify_output_pin(&n.kind, tap_pin, registry))
}

/// Build and inject the preview subgraph into the running pipeline.
///
/// Accepts one or more tap points (e.g. one audio + one video) and
/// creates the appropriate encoding chains, all feeding a single
/// moq_peer node.
///
/// On failure, rolls back any nodes/connections that were successfully
/// created before the error.
///
/// Returns `(injected_nodes, injected_connections, has_audio, has_video)`
/// where each injected node is a `(node_id, kind)` tuple.
///
/// # Errors
///
/// Returns an error if a tap node is missing from the pipeline, if no
/// media types are detected, or if any engine control message fails.
pub async fn inject_preview_subgraph(
    session: &crate::session::Session,
    preview_id: &str,
    tap_points: &[TapPoint],
    gateway_path: &str,
    pipeline: &Pipeline,
) -> Result<
    (
        Vec<(String, String)>,
        Vec<(String, String, String, String, streamkit_core::control::ConnectionMode)>,
        bool,
        bool,
    ),
    String,
> {
    let prefix = format!("_preview_{preview_id}_");

    // Validate all tap points exist in the pipeline.
    for tp in tap_points {
        if !pipeline.nodes.contains_key(&tp.node) {
            return Err(format!("Tap node '{}' not found in pipeline", tp.node));
        }
    }

    let has_audio = tap_points.iter().any(|tp| tp.is_audio);
    let has_video = tap_points.iter().any(|tp| tp.is_video);

    if !has_audio && !has_video {
        return Err("Tap points do not produce audio or video".to_string());
    }

    let mut node_ids: Vec<(String, String)> = Vec::new();
    let mut connections: Vec<(
        String,
        String,
        String,
        String,
        streamkit_core::control::ConnectionMode,
    )> = Vec::new();

    // Determine moq_peer input pin names.
    // When the preview has both audio and video, moq_peer expects audio
    // on "in" and video on "in_1".  When only one media type is present,
    // the single stream goes to "in".
    let peer_node_id = format!("{prefix}peer");
    let peer_audio_input = "in";
    let peer_video_input = if has_audio && has_video { "in_1" } else { "in" };

    // Helper: on any engine error, tear down what we've built so far
    // and propagate the error.
    let result = inject_subgraph_inner(
        session,
        tap_points,
        &prefix,
        gateway_path,
        &peer_node_id,
        peer_audio_input,
        peer_video_input,
        has_audio,
        has_video,
        &mut node_ids,
        &mut connections,
    )
    .await;

    if let Err(e) = result {
        // Roll back: tear down any partially-created subgraph.
        let partial = PreviewState {
            preview_id: preview_id.to_string(),
            tap_points: tap_points.to_vec(),
            injected_nodes: node_ids,
            injected_connections: connections,
            gateway_path: gateway_path.to_string(),
            has_audio,
            has_video,
            created_at: std::time::SystemTime::now(),
        };
        tracing::warn!(
            preview_id = %preview_id,
            error = %e,
            "Rolling back partially-injected preview subgraph"
        );
        // Best-effort rollback — ignore teardown errors.
        let _ = teardown_preview(session, &partial).await;
        return Err(e);
    }

    Ok((node_ids, connections, has_audio, has_video))
}

/// Inner implementation of subgraph injection. Separated so that the
/// caller can roll back `node_ids` and `connections` on error.
///
/// **Assumption**: `tap_points` contains at most one audio and one video
/// tap point. Multiple same-type taps would collide on node IDs (e.g.
/// two raw video taps would both create `{prefix}pixconv`). The public
/// `inject_preview_subgraph` upholds this via `detect_tap_points`
/// deduplication.
#[allow(clippy::too_many_arguments)]
async fn inject_subgraph_inner(
    session: &crate::session::Session,
    tap_points: &[TapPoint],
    prefix: &str,
    gateway_path: &str,
    peer_node_id: &str,
    peer_audio_input: &str,
    peer_video_input: &str,
    has_audio: bool,
    has_video: bool,
    node_ids: &mut Vec<(String, String)>,
    connections: &mut Vec<(
        String,
        String,
        String,
        String,
        streamkit_core::control::ConnectionMode,
    )>,
) -> Result<(), String> {
    // Add encoder chains for each tap point BEFORE adding the moq_peer
    // (the peer must be added last so it sees its inputs).
    for tp in tap_points {
        if tp.is_encoded {
            // Encoded — will connect directly to peer (added below)
        } else if tp.is_video {
            // Raw video path: pixel_convert → vp9_encoder
            let pixconv_id = format!("{prefix}pixconv");
            let vp9enc_id = format!("{prefix}vp9enc");

            add_pixel_convert_node(session, &pixconv_id).await?;
            node_ids.push((pixconv_id.clone(), "video::pixel_convert".to_string()));

            add_vp9_encoder_node(session, &vp9enc_id).await?;
            node_ids.push((vp9enc_id.clone(), "vp9::encoder".to_string()));

            // tap → pixconv
            connect_best_effort(session, &tp.node, &tp.pin, &pixconv_id, "in", connections).await?;
            // pixconv → vp9enc
            connect_reliable(session, &pixconv_id, "out", &vp9enc_id, "in", connections).await?;
        } else if tp.is_audio && !tp.is_encoded {
            // Raw audio path: opus_encoder
            let opusenc_id = format!("{prefix}opusenc");

            add_opus_encoder_node(session, &opusenc_id).await?;
            node_ids.push((opusenc_id.clone(), "audio::opus::encoder".to_string()));

            // tap → opusenc
            connect_best_effort(session, &tp.node, &tp.pin, &opusenc_id, "in", connections).await?;
        }
    }

    // Add moq_peer node
    add_moq_peer_node(session, peer_node_id, gateway_path).await?;
    node_ids.push((peer_node_id.to_string(), "transport::moq::peer".to_string()));

    // Connect encoder outputs (or direct taps) to moq_peer
    for tp in tap_points {
        if tp.is_encoded && tp.is_audio {
            connect_best_effort(
                session,
                &tp.node,
                &tp.pin,
                peer_node_id,
                peer_audio_input,
                connections,
            )
            .await?;
        } else if tp.is_encoded && tp.is_video {
            connect_best_effort(
                session,
                &tp.node,
                &tp.pin,
                peer_node_id,
                peer_video_input,
                connections,
            )
            .await?;
        } else if tp.is_video {
            // Connect vp9enc → peer
            let vp9enc_id = format!("{prefix}vp9enc");
            connect_reliable(
                session,
                &vp9enc_id,
                "out",
                peer_node_id,
                peer_video_input,
                connections,
            )
            .await?;
        } else if tp.is_audio {
            // Connect opusenc → peer
            let opusenc_id = format!("{prefix}opusenc");
            connect_reliable(
                session,
                &opusenc_id,
                "out",
                peer_node_id,
                peer_audio_input,
                connections,
            )
            .await?;
        }
    }

    // The caller guarantees at least one media type is present
    // (it returns Err before reaching here if both are false).
    debug_assert!(has_audio || has_video, "inject_subgraph_inner called with no media types");

    Ok(())
}

// ── Node creation helpers ────────────────────────────────────────────

async fn add_moq_peer_node(
    session: &crate::session::Session,
    node_id: &str,
    gateway_path: &str,
) -> Result<(), String> {
    let params = serde_json::json!({
        "gateway_path": gateway_path,
        "output_broadcast": "output",
        "input_broadcasts": ["input"],
        "allow_reconnect": false,
        "output_group_duration_ms": 40,
    });
    session
        .try_send_control_message(EngineControlMessage::AddNode {
            node_id: node_id.to_string(),
            kind: "transport::moq::peer".to_string(),
            params: Some(params),
        })
        .await
}

async fn add_vp9_encoder_node(
    session: &crate::session::Session,
    node_id: &str,
) -> Result<(), String> {
    let params = serde_json::json!({
        "bitrate_kbps": 1000,
        "keyframe_interval": 60,
        "threads": 1,
        "deadline": "realtime",
        "cpu_used": 8,
    });
    session
        .try_send_control_message(EngineControlMessage::AddNode {
            node_id: node_id.to_string(),
            kind: "video::vp9::encoder".to_string(),
            params: Some(params),
        })
        .await
}

async fn add_opus_encoder_node(
    session: &crate::session::Session,
    node_id: &str,
) -> Result<(), String> {
    let params = serde_json::json!({
        "bitrate": 48000,
    });
    session
        .try_send_control_message(EngineControlMessage::AddNode {
            node_id: node_id.to_string(),
            kind: "audio::opus::encoder".to_string(),
            params: Some(params),
        })
        .await
}

async fn add_pixel_convert_node(
    session: &crate::session::Session,
    node_id: &str,
) -> Result<(), String> {
    let params = serde_json::json!({
        "output_format": "i420",
    });
    session
        .try_send_control_message(EngineControlMessage::AddNode {
            node_id: node_id.to_string(),
            kind: "video::pixel_convert".to_string(),
            params: Some(params),
        })
        .await
}

// ── Connection helpers ───────────────────────────────────────────────

async fn connect_best_effort(
    session: &crate::session::Session,
    from_node: &str,
    from_pin: &str,
    to_node: &str,
    to_pin: &str,
    connections: &mut Vec<(
        String,
        String,
        String,
        String,
        streamkit_core::control::ConnectionMode,
    )>,
) -> Result<(), String> {
    let mode = streamkit_core::control::ConnectionMode::BestEffort;
    session
        .try_send_control_message(EngineControlMessage::Connect {
            from_node: from_node.to_string(),
            from_pin: from_pin.to_string(),
            to_node: to_node.to_string(),
            to_pin: to_pin.to_string(),
            mode,
        })
        .await?;
    connections.push((
        from_node.to_string(),
        from_pin.to_string(),
        to_node.to_string(),
        to_pin.to_string(),
        mode,
    ));
    Ok(())
}

async fn connect_reliable(
    session: &crate::session::Session,
    from_node: &str,
    from_pin: &str,
    to_node: &str,
    to_pin: &str,
    connections: &mut Vec<(
        String,
        String,
        String,
        String,
        streamkit_core::control::ConnectionMode,
    )>,
) -> Result<(), String> {
    let mode = streamkit_core::control::ConnectionMode::Reliable;
    session
        .try_send_control_message(EngineControlMessage::Connect {
            from_node: from_node.to_string(),
            from_pin: from_pin.to_string(),
            to_node: to_node.to_string(),
            to_pin: to_pin.to_string(),
            mode,
        })
        .await?;
    connections.push((
        from_node.to_string(),
        from_pin.to_string(),
        to_node.to_string(),
        to_pin.to_string(),
        mode,
    ));
    Ok(())
}

// ── Teardown ─────────────────────────────────────────────────────────

/// Tear down the preview subgraph (disconnect, then remove nodes in reverse).
/// Also removes the preview nodes and connections from `session.pipeline`
/// so the pipeline API stays in sync.
///
/// Uses fallible sends so the caller can detect partial teardown failures.
/// Returns `Ok(())` on full success, or `Err(msg)` if any engine message
/// failed (the pipeline model is still cleaned up regardless).
///
/// # Errors
///
/// Returns the first engine control message error encountered during
/// teardown. The pipeline model is cleaned up regardless of errors.
pub async fn teardown_preview(
    session: &crate::session::Session,
    state: &PreviewState,
) -> Result<(), String> {
    let mut first_error: Option<String> = None;

    // Disconnect in reverse order
    for (from_node, from_pin, to_node, to_pin, _mode) in state.injected_connections.iter().rev() {
        if let Err(e) = session
            .try_send_control_message(EngineControlMessage::Disconnect {
                from_node: from_node.clone(),
                from_pin: from_pin.clone(),
                to_node: to_node.clone(),
                to_pin: to_pin.clone(),
            })
            .await
        {
            tracing::warn!(error = %e, "Failed to disconnect preview node");
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }
    // Remove nodes in reverse order (peer first, then encoders)
    for (node_id, _kind) in state.injected_nodes.iter().rev() {
        if let Err(e) = session
            .try_send_control_message(EngineControlMessage::RemoveNode { node_id: node_id.clone() })
            .await
        {
            tracing::warn!(error = %e, node_id = %node_id, "Failed to remove preview node");
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    // Sync the pipeline model: remove preview nodes and connections
    // regardless of engine errors.
    {
        let mut pipeline = session.pipeline.lock().await;
        for (node_id, _kind) in &state.injected_nodes {
            pipeline.nodes.shift_remove(node_id);
        }
        pipeline.connections.retain(|conn| {
            !state.injected_connections.iter().any(|(f, fp, t, tp, _mode)| {
                conn.from_node == *f
                    && conn.from_pin == *fp
                    && conn.to_node == *t
                    && conn.to_pin == *tp
            })
        });
    }

    first_error.map_or(Ok(()), Err)
}

/// Tear down all active previews for a session.
pub async fn teardown_all_previews(session: &crate::session::Session) {
    let previews = session.list_previews().await;
    for preview in &previews {
        tracing::debug!(
            session_id = %session.id,
            preview_id = %preview.preview_id,
            "Tearing down preview on session destroy"
        );
        // Best-effort teardown on session destroy.
        let _ = teardown_preview(session, preview).await;
    }
}

// ── Route handlers ───────────────────────────────────────────────────

/// POST /api/v1/sessions/{id}/preview
///
/// # Errors
///
/// Returns HTTP error status codes for permission, validation, or engine failures.
pub async fn start_preview_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<StartPreviewRequest>,
) -> Result<(StatusCode, Json<PreviewResponse>), (StatusCode, String)> {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.modify_sessions {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot create previews".to_string(),
        ));
    }

    // Require MoQ gateway
    if app_state.moq_gateway.is_none() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Preview requires MoQ gateway to be enabled".to_string(),
        ));
    }

    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    };

    let Some(session) = session else {
        return Err((StatusCode::NOT_FOUND, format!("Session '{session_id}' not found")));
    };

    // Check ownership
    if !perms.access_all_sessions && session.created_by.as_ref().is_some_and(|c| c != &role_name) {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: you do not own this session".to_string(),
        ));
    }

    // Optimistic check — prevents unnecessary subgraph injection when
    // the limit is clearly exceeded.  The authoritative check happens in
    // `add_preview()` under the lock; if two concurrent requests both
    // pass this point, the loser's subgraph is rolled back by the
    // teardown_preview call below.
    if session.preview_count().await >= MAX_PREVIEWS_PER_SESSION {
        return Err((
            StatusCode::CONFLICT,
            format!("Maximum of {MAX_PREVIEWS_PER_SESSION} concurrent previews per session"),
        ));
    }

    let pipeline = session.pipeline.lock().await.clone();

    // Resolve tap points. When a specific node is given, create a single
    // tap point from it. Otherwise auto-detect from the pipeline graph.
    let tap_points = match (req.tap_node, req.tap_pin) {
        (Some(node), Some(pin)) => {
            if !pipeline.nodes.contains_key(&node) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Tap node '{node}' not found in pipeline"),
                ));
            }
            // Validate the pin exists on this node.
            let registry = read_registry(&app_state)
                .map_err(|status| (status, "Engine registry unavailable".to_string()))?;
            if let Some(def) = registry.get_definition(&pipeline.nodes[&node].kind) {
                if !def.outputs.iter().any(|p| p.name == pin) {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        format!("Pin '{pin}' not found on node '{node}'"),
                    ));
                }
            } else {
                tracing::warn!(
                    node = %node,
                    kind = %pipeline.nodes[&node].kind,
                    pin = %pin,
                    "Node kind not found in registry — skipping pin validation"
                );
            }
            let classification =
                classify_output_pin_from_registry(&pipeline, &node, &pin, &registry);
            drop(registry);
            let (is_encoded, is_audio, is_video) = classification;
            vec![TapPoint { node, pin, is_encoded, is_audio, is_video }]
        },
        (Some(node), None) => {
            if !pipeline.nodes.contains_key(&node) {
                return Err((
                    StatusCode::BAD_REQUEST,
                    format!("Tap node '{node}' not found in pipeline"),
                ));
            }
            // Look up the node's first output pin from the registry
            // instead of blindly assuming "out".
            let (pin, classification) = {
                let registry = read_registry(&app_state)
                    .map_err(|status| (status, "Engine registry unavailable".to_string()))?;
                let node_kind = &pipeline.nodes[&node].kind;
                let pin_name = if let Some(def) = registry.get_definition(node_kind) {
                    def.outputs.first().map_or_else(|| "out".to_string(), |p| p.name.clone())
                } else {
                    "out".to_string()
                };
                let cls = classify_output_pin_from_registry(&pipeline, &node, &pin_name, &registry);
                drop(registry);
                (pin_name, cls)
            };
            let (is_encoded, is_audio, is_video) = classification;
            vec![TapPoint { node, pin, is_encoded, is_audio, is_video }]
        },
        (None, Some(_)) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "tap_pin requires tap_node to be specified".to_string(),
            ));
        },
        (None, None) => {
            let registry = read_registry(&app_state)
                .map_err(|status| (status, "Engine registry unavailable".to_string()))?;
            detect_tap_points(&pipeline, &registry).map_err(|e| (StatusCode::BAD_REQUEST, e))?
        },
    };

    let preview_id = uuid::Uuid::new_v4().to_string();
    let gateway_path = format!("/_preview/{}/{}", session.id, preview_id);

    let (nodes, connections, has_audio, has_video) =
        inject_preview_subgraph(&session, &preview_id, &tap_points, &gateway_path, &pipeline)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Sync the pipeline model so the pipeline API includes preview nodes.
    {
        let mut pip = session.pipeline.lock().await;
        for (node_id, kind) in &nodes {
            pip.nodes.insert(
                node_id.clone(),
                streamkit_api::Node { kind: kind.clone(), params: None, state: None },
            );
        }
        for (from_node, from_pin, to_node, to_pin, mode) in &connections {
            pip.connections.push(streamkit_api::Connection {
                from_node: from_node.clone(),
                from_pin: from_pin.clone(),
                to_node: to_node.clone(),
                to_pin: to_pin.clone(),
                mode: *mode,
            });
        }
    }

    let state = PreviewState {
        preview_id: preview_id.clone(),
        tap_points,
        injected_nodes: nodes,
        injected_connections: connections,
        gateway_path: gateway_path.clone(),
        has_audio,
        has_video,
        created_at: std::time::SystemTime::now(),
    };

    if let Err(e) = session.add_preview(state.clone()).await {
        // Another request won the race — clean up the nodes we just injected.
        // Best-effort teardown for TOCTOU race loser.
        let _ = teardown_preview(&session, &state).await;
        return Err((StatusCode::CONFLICT, e));
    }

    let tap_summary: Vec<String> = state
        .tap_points
        .iter()
        .map(|t| {
            let media = match (t.is_audio, t.is_video) {
                (true, true) => "audio+video",
                (true, false) => "audio",
                _ => "video",
            };
            format!("{}:{} ({})", t.node, t.pin, media)
        })
        .collect();

    info!(
        session_id = %session.id,
        preview_id = %preview_id,
        gateway_path = %gateway_path,
        audio = has_audio,
        video = has_video,
        tap_points = ?tap_summary,
        "Started preview"
    );

    Ok((
        StatusCode::CREATED,
        Json(PreviewResponse {
            preview_id,
            gateway_path,
            broadcast: "output".to_string(),
            audio: has_audio,
            video: has_video,
        }),
    ))
}

/// GET /api/v1/sessions/{id}/preview
///
/// # Errors
///
/// Returns HTTP 403 if the caller lacks permission.
pub async fn list_previews_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<PreviewInfo>>, (StatusCode, String)> {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.list_sessions {
        return Err((StatusCode::FORBIDDEN, "Permission denied: cannot list previews".to_string()));
    }

    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    };

    let Some(session) = session else {
        return Err((StatusCode::NOT_FOUND, format!("Session '{session_id}' not found")));
    };

    if !perms.access_all_sessions && session.created_by.as_ref().is_some_and(|c| c != &role_name) {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: you do not own this session".to_string(),
        ));
    }

    let previews = session.list_previews().await;
    let infos: Vec<PreviewInfo> = previews
        .into_iter()
        .map(|p| {
            let primary_tap = p.tap_points.first();
            PreviewInfo {
                preview_id: p.preview_id,
                gateway_path: p.gateway_path,
                broadcast: "output".to_string(),
                audio: p.has_audio,
                video: p.has_video,
                tap_node: primary_tap.map_or_else(String::new, |t| t.node.clone()),
                tap_pin: primary_tap.map_or_else(String::new, |t| t.pin.clone()),
                tap_points: p
                    .tap_points
                    .iter()
                    .map(|t| TapPointInfo {
                        node: t.node.clone(),
                        pin: t.pin.clone(),
                        media: match (t.is_audio, t.is_video) {
                            (true, true) => "audio+video",
                            (true, false) => "audio",
                            _ => "video",
                        }
                        .to_string(),
                    })
                    .collect(),
                created_at: system_time_to_rfc3339(p.created_at),
            }
        })
        .collect();

    Ok(Json(infos))
}

/// DELETE /api/v1/sessions/{id}/preview/{preview_id}
///
/// # Errors
///
/// Returns HTTP 404 if the session or preview is not found, or 403 on
/// permission denial. Returns 500 if teardown partially fails.
pub async fn stop_preview_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session_id, preview_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);

    if !perms.modify_sessions {
        return Err((StatusCode::FORBIDDEN, "Permission denied: cannot stop previews".to_string()));
    }

    let session = {
        let session_manager = app_state.session_manager.lock().await;
        session_manager.get_session_by_name_or_id(&session_id)
    };

    let Some(session) = session else {
        return Err((StatusCode::NOT_FOUND, format!("Session '{session_id}' not found")));
    };

    if !perms.access_all_sessions && session.created_by.as_ref().is_some_and(|c| c != &role_name) {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: you do not own this session".to_string(),
        ));
    }

    let Some(preview_state) = session.remove_preview(&preview_id).await else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("Preview '{preview_id}' not found on session '{session_id}'"),
        ));
    };

    if let Err(e) = teardown_preview(&session, &preview_state).await {
        tracing::warn!(
            session_id = %session.id,
            preview_id = %preview_id,
            error = %e,
            "Partial teardown failure during explicit stop"
        );
    }

    info!(
        session_id = %session.id,
        preview_id = %preview_id,
        "Stopped preview"
    );

    Ok(Json(serde_json::json!({ "preview_id": preview_id })))
}
