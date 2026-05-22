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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Panics ARE the assertions
mod tests {
    use super::*;
    use streamkit_api::{Connection, Node, Pipeline};
    use streamkit_core::registry::StaticPins;
    use streamkit_core::types::{
        AudioCodec, AudioFormat, EncodedAudioFormat, EncodedVideoFormat, PixelFormat,
        RawVideoFormat, SampleFormat, VideoCodec,
    };
    use streamkit_core::{NodeRegistry, OutputPin, PinCardinality, ProcessorNode, StreamKitError};

    /// Stub factory used only to satisfy `register_static`'s `Fn` bound.
    /// `get_definition` reads `static_pins` directly and never invokes the
    /// factory in these tests.
    fn stub_factory(
        _: Option<&serde_json::Value>,
    ) -> Result<Box<dyn ProcessorNode>, StreamKitError> {
        Err(StreamKitError::Configuration("test stub: factory not invokable".into()))
    }

    fn audio_format() -> AudioFormat {
        AudioFormat { sample_rate: 48_000, channels: 2, sample_format: SampleFormat::F32 }
    }

    fn raw_video_format() -> RawVideoFormat {
        RawVideoFormat { width: Some(1280), height: Some(720), pixel_format: PixelFormat::I420 }
    }

    fn opus_format() -> EncodedAudioFormat {
        EncodedAudioFormat { codec: AudioCodec::Opus, codec_private: None }
    }

    fn vp9_format() -> EncodedVideoFormat {
        EncodedVideoFormat {
            codec: VideoCodec::Vp9,
            bitstream_format: None,
            codec_private: None,
            profile: None,
            level: None,
        }
    }

    fn register_with_output(
        registry: &mut NodeRegistry,
        kind: &str,
        out_pin: &str,
        produces: PacketType,
    ) {
        registry.register_static(
            kind,
            stub_factory,
            serde_json::json!({}),
            StaticPins {
                inputs: vec![],
                outputs: vec![OutputPin {
                    name: out_pin.into(),
                    produces_type: produces,
                    cardinality: PinCardinality::Broadcast,
                }],
            },
            vec![],
            false,
        );
    }

    fn node(kind: &str) -> Node {
        Node { kind: kind.to_string(), params: None, state: None }
    }

    fn connection(from_node: &str, from_pin: &str, to_node: &str, to_pin: &str) -> Connection {
        Connection {
            from_node: from_node.to_string(),
            from_pin: from_pin.to_string(),
            to_node: to_node.to_string(),
            to_pin: to_pin.to_string(),
            mode: streamkit_core::control::ConnectionMode::Reliable,
        }
    }

    fn pipeline_with(nodes: &[(&str, &str)], connections: Vec<Connection>) -> Pipeline {
        let mut p = Pipeline::default();
        for (id, kind) in nodes {
            p.nodes.insert((*id).to_string(), node(kind));
        }
        p.connections = connections;
        p
    }

    #[test]
    fn is_terminal_kind_returns_true_for_documented_sinks() {
        assert!(is_terminal_kind("transport::moq::publisher"));
        assert!(is_terminal_kind("transport::moq::peer"));
        assert!(is_terminal_kind("core::sink"));
        assert!(is_terminal_kind("io::file_writer"));
    }

    #[test]
    fn is_terminal_kind_returns_false_for_sources_and_transforms() {
        assert!(!is_terminal_kind("audio::opus::encoder"));
        assert!(!is_terminal_kind("video::vp9::encoder"));
        assert!(!is_terminal_kind("audio::gain"));
        assert!(!is_terminal_kind("video::compositor"));
        assert!(!is_terminal_kind(""));
        assert!(!is_terminal_kind("transport::moq"));
        assert!(!is_terminal_kind("transport::moq::subscriber"));
    }

    #[test]
    fn classify_by_kind_table_driven() {
        let cases: &[(&str, (bool, bool, bool))] = &[
            ("audio::opus::encoder", (true, true, false)),
            ("audio::aac::encoder", (true, true, false)),
            ("video::vp9::encoder", (true, false, true)),
            ("video::h264::encoder", (true, false, true)),
            ("video::compositor", (false, false, true)),
            ("video::pixel_convert", (false, false, true)),
            ("audio::mixer", (false, true, false)),
            ("audio::resampler", (false, true, false)),
            ("audio::gain", (false, true, false)),
            ("streamkit::widget", (false, false, false)),
            ("custom::node", (false, false, false)),
            ("", (false, false, false)),
        ];

        for (kind, expected) in cases {
            assert_eq!(classify_by_kind(kind), *expected, "classify_by_kind({kind:?}) mismatch");
        }
    }

    #[test]
    fn classify_output_pin_uses_registry_when_kind_is_known() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );
        register_with_output(
            &mut registry,
            "video::vp9::encoder",
            "out",
            PacketType::EncodedVideo(vp9_format()),
        );
        register_with_output(
            &mut registry,
            "audio::source",
            "out",
            PacketType::RawAudio(audio_format()),
        );
        register_with_output(
            &mut registry,
            "video::source",
            "out",
            PacketType::RawVideo(raw_video_format()),
        );

        assert_eq!(
            classify_output_pin("audio::opus::encoder", "out", &registry),
            (true, true, false),
        );
        assert_eq!(
            classify_output_pin("video::vp9::encoder", "out", &registry),
            (true, false, true),
        );
        assert_eq!(classify_output_pin("audio::source", "out", &registry), (false, true, false),);
        assert_eq!(classify_output_pin("video::source", "out", &registry), (false, false, true),);
    }

    #[test]
    fn classify_output_pin_falls_back_to_kind_when_kind_unknown() {
        let registry = NodeRegistry::new();
        assert_eq!(
            classify_output_pin("audio::opus::encoder", "out", &registry),
            (true, true, false),
        );
        assert_eq!(
            classify_output_pin("video::vp9::encoder", "out", &registry),
            (true, false, true),
        );
        assert_eq!(classify_output_pin("plugin::custom", "out", &registry), (false, false, false),);
    }

    #[test]
    fn classify_output_pin_falls_back_when_pin_name_does_not_match() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );

        // Kind is registered but pin name is missing — kind heuristic still
        // recognises this as an audio encoder.
        assert_eq!(
            classify_output_pin("audio::opus::encoder", "nonexistent", &registry),
            (true, true, false),
        );
    }

    #[test]
    fn classify_output_pin_passthrough_pin_uses_kind_heuristic() {
        let mut registry = NodeRegistry::new();
        registry.register_static(
            "video::passthrough",
            stub_factory,
            serde_json::json!({}),
            StaticPins {
                inputs: vec![],
                outputs: vec![OutputPin {
                    name: "out".into(),
                    produces_type: PacketType::Any,
                    cardinality: PinCardinality::Broadcast,
                }],
            },
            vec![],
            false,
        );

        // `PacketType::Any` is not classifiable by the registry path, so the
        // call falls through to the kind heuristic, which doesn't match
        // "passthrough" → unclassified.
        assert_eq!(
            classify_output_pin("video::passthrough", "out", &registry),
            (false, false, false),
        );
    }

    #[test]
    fn classify_output_pin_from_registry_returns_false_when_tap_node_missing() {
        let registry = NodeRegistry::new();
        let pipeline = pipeline_with(&[("enc", "audio::opus::encoder")], vec![]);

        assert_eq!(
            classify_output_pin_from_registry(&pipeline, "missing", "out", &registry),
            (false, false, false),
        );
    }

    #[test]
    fn classify_output_pin_from_registry_matches_classify_output_pin_when_node_present() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );
        let pipeline = pipeline_with(&[("enc", "audio::opus::encoder")], vec![]);

        assert_eq!(
            classify_output_pin_from_registry(&pipeline, "enc", "out", &registry),
            classify_output_pin("audio::opus::encoder", "out", &registry),
        );
    }

    #[test]
    fn detect_tap_points_single_audio_encoder_to_moq_peer() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );

        let pipeline = pipeline_with(
            &[("enc", "audio::opus::encoder"), ("peer", "transport::moq::peer")],
            vec![connection("enc", "out", "peer", "in")],
        );

        let taps = detect_tap_points(&pipeline, &registry).unwrap();
        assert_eq!(taps.len(), 1);
        assert_eq!(taps[0].node, "enc");
        assert_eq!(taps[0].pin, "out");
        assert!(taps[0].is_encoded);
        assert!(taps[0].is_audio);
        assert!(!taps[0].is_video);
    }

    #[test]
    fn detect_tap_points_audio_and_video_encoders_to_same_peer() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );
        register_with_output(
            &mut registry,
            "video::vp9::encoder",
            "out",
            PacketType::EncodedVideo(vp9_format()),
        );

        let pipeline = pipeline_with(
            &[
                ("aenc", "audio::opus::encoder"),
                ("venc", "video::vp9::encoder"),
                ("peer", "transport::moq::peer"),
            ],
            vec![
                connection("aenc", "out", "peer", "in"),
                connection("venc", "out", "peer", "in_1"),
            ],
        );

        let taps = detect_tap_points(&pipeline, &registry).unwrap();
        assert_eq!(taps.len(), 2);

        let audio = taps.iter().find(|t| t.is_audio).expect("audio tap present");
        let video = taps.iter().find(|t| t.is_video).expect("video tap present");
        assert_eq!(audio.node, "aenc");
        assert!(audio.is_encoded);
        assert!(!audio.is_video);
        assert_eq!(video.node, "venc");
        assert!(video.is_encoded);
        assert!(!video.is_audio);
    }

    #[test]
    fn detect_tap_points_raw_video_source_to_moq_peer() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "video::source",
            "out",
            PacketType::RawVideo(raw_video_format()),
        );

        let pipeline = pipeline_with(
            &[("src", "video::source"), ("peer", "transport::moq::peer")],
            vec![connection("src", "out", "peer", "in")],
        );

        let taps = detect_tap_points(&pipeline, &registry).unwrap();
        assert_eq!(taps.len(), 1);
        assert_eq!(taps[0].node, "src");
        assert!(!taps[0].is_encoded);
        assert!(taps[0].is_video);
        assert!(!taps[0].is_audio);
    }

    #[test]
    fn detect_tap_points_prefers_encoded_when_both_raw_and_encoded_present() {
        // Raw + encoded video both feed the same moq_peer; the encoded path
        // wins so the preview avoids re-encoding.
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "video::vp9::encoder",
            "out",
            PacketType::EncodedVideo(vp9_format()),
        );
        register_with_output(
            &mut registry,
            "video::source",
            "out",
            PacketType::RawVideo(raw_video_format()),
        );

        let pipeline = pipeline_with(
            &[
                ("raw", "video::source"),
                ("enc", "video::vp9::encoder"),
                ("peer", "transport::moq::peer"),
            ],
            vec![connection("raw", "out", "peer", "in_1"), connection("enc", "out", "peer", "in")],
        );

        let taps = detect_tap_points(&pipeline, &registry).unwrap();
        assert_eq!(taps.len(), 1);
        assert_eq!(taps[0].node, "enc");
        assert!(taps[0].is_encoded);
    }

    #[test]
    fn detect_tap_points_fallback_picks_first_non_terminal_source_as_video() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );

        // No terminal sink — pipeline ends at a non-terminal node.
        let pipeline = pipeline_with(
            &[("enc", "audio::opus::encoder"), ("transform", "audio::gain")],
            vec![connection("enc", "out", "transform", "in")],
        );

        let taps = detect_tap_points(&pipeline, &registry).unwrap();
        assert_eq!(taps.len(), 1);
        assert_eq!(taps[0].node, "enc");
        assert_eq!(taps[0].pin, "out");
        assert!(!taps[0].is_encoded);
        assert!(!taps[0].is_audio);
        assert!(taps[0].is_video, "fallback path is documented to assume video");
    }

    #[test]
    fn detect_tap_points_empty_pipeline_returns_err() {
        let registry = NodeRegistry::new();
        let pipeline = Pipeline::default();

        let err = detect_tap_points(&pipeline, &registry).unwrap_err();
        assert!(err.starts_with("Cannot auto-detect tap point"), "unexpected error: {err}");
    }

    #[test]
    fn detect_tap_points_dedups_two_audio_encoders_into_one_audio_tap() {
        let mut registry = NodeRegistry::new();
        register_with_output(
            &mut registry,
            "audio::opus::encoder",
            "out",
            PacketType::EncodedAudio(opus_format()),
        );

        let pipeline = pipeline_with(
            &[
                ("enc_a", "audio::opus::encoder"),
                ("enc_b", "audio::opus::encoder"),
                ("peer", "transport::moq::peer"),
            ],
            vec![
                connection("enc_a", "out", "peer", "in"),
                connection("enc_b", "out", "peer", "in_1"),
            ],
        );

        let taps = detect_tap_points(&pipeline, &registry).unwrap();
        assert_eq!(taps.len(), 1, "duplicate audio encoders must collapse to one tap");
        assert!(taps[0].is_audio);
        assert!(!taps[0].is_video);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Panics ARE the assertions
mod inject_teardown_tests {
    use super::*;
    use crate::config::Config;
    use crate::session::{PreviewState, Session, TapPoint};
    use crate::state::BroadcastEvent;
    use std::time::SystemTime;
    use streamkit_engine::Engine;
    use tokio::sync::broadcast;

    async fn fresh_session() -> (Session, broadcast::Receiver<BroadcastEvent>) {
        let engine = Engine::without_plugins();
        let config = Config::default();
        let (tx, rx) = broadcast::channel(16);
        let session = Session::create(
            &engine,
            &config,
            Some("preview-inject-test".to_string()),
            tx,
            Some("test-role".to_string()),
        )
        .await
        .expect("Session::create on a fresh engine should succeed");
        (session, rx)
    }

    fn tap(node: &str, pin: &str, is_encoded: bool, is_audio: bool, is_video: bool) -> TapPoint {
        TapPoint { node: node.to_string(), pin: pin.to_string(), is_encoded, is_audio, is_video }
    }

    async fn seed_pipeline_node(session: &Session, node_id: &str, kind: &str) {
        let mut pipeline = session.pipeline.lock().await;
        pipeline.nodes.insert(
            node_id.to_string(),
            streamkit_api::Node { kind: kind.to_string(), params: None, state: None },
        );
    }

    fn empty_pipeline() -> streamkit_api::Pipeline {
        streamkit_api::Pipeline::default()
    }

    #[tokio::test]
    async fn inject_returns_err_when_tap_node_missing_in_pipeline() {
        let (session, _rx) = fresh_session().await;
        let taps = vec![tap("ghost", "out", true, false, true)];

        let err = inject_preview_subgraph(&session, "pv1", &taps, "/gw/path", &empty_pipeline())
            .await
            .unwrap_err();
        assert!(err.contains("Tap node 'ghost' not found"), "unexpected error: {err}");

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn inject_returns_err_when_tap_points_have_no_media() {
        let (session, _rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "src".to_string(),
            streamkit_api::Node { kind: "custom".to_string(), params: None, state: None },
        );
        let taps = vec![tap("src", "out", false, false, false)];

        let err = inject_preview_subgraph(&session, "pv1", &taps, "/gw/path", &pipeline)
            .await
            .unwrap_err();
        assert!(err.contains("do not produce audio or video"), "unexpected error: {err}");

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn inject_encoded_audio_only_creates_peer_and_single_connection() {
        let (session, _rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "aenc".to_string(),
            streamkit_api::Node {
                kind: "audio::opus::encoder".to_string(),
                params: None,
                state: None,
            },
        );
        let taps = vec![tap("aenc", "out", true, true, false)];

        let (nodes, conns, has_audio, has_video) =
            inject_preview_subgraph(&session, "pv1", &taps, "/gw/pv1", &pipeline)
                .await
                .expect("inject should succeed for encoded audio");

        assert!(has_audio);
        assert!(!has_video);

        assert_eq!(nodes.len(), 1, "only the moq peer should be injected for encoded audio");
        assert_eq!(nodes[0].0, "_preview_pv1_peer");
        assert_eq!(nodes[0].1, "transport::moq::peer");

        assert_eq!(conns.len(), 1);
        let (from_node, from_pin, to_node, to_pin, mode) = &conns[0];
        assert_eq!(from_node, "aenc");
        assert_eq!(from_pin, "out");
        assert_eq!(to_node, "_preview_pv1_peer");
        assert_eq!(to_pin, "in");
        assert!(matches!(*mode, streamkit_core::control::ConnectionMode::BestEffort));

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn inject_encoded_audio_and_video_routes_video_to_in_1() {
        let (session, _rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "aenc".to_string(),
            streamkit_api::Node {
                kind: "audio::opus::encoder".to_string(),
                params: None,
                state: None,
            },
        );
        pipeline.nodes.insert(
            "venc".to_string(),
            streamkit_api::Node {
                kind: "video::vp9::encoder".to_string(),
                params: None,
                state: None,
            },
        );
        let taps =
            vec![tap("aenc", "out", true, true, false), tap("venc", "out", true, false, true)];

        let (nodes, conns, has_audio, has_video) =
            inject_preview_subgraph(&session, "pv1", &taps, "/gw/pv1", &pipeline)
                .await
                .expect("inject should succeed");

        assert!(has_audio && has_video);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].1, "transport::moq::peer");

        let audio_conn =
            conns.iter().find(|(f, _, _, _, _)| f == "aenc").expect("audio connection present");
        assert_eq!(audio_conn.3, "in");

        let video_conn =
            conns.iter().find(|(f, _, _, _, _)| f == "venc").expect("video connection present");
        assert_eq!(video_conn.3, "in_1");

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn inject_raw_video_creates_pixconv_and_vp9_encoder() {
        let (session, _rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "src".to_string(),
            streamkit_api::Node { kind: "video::source".to_string(), params: None, state: None },
        );
        let taps = vec![tap("src", "out", false, false, true)];

        let (nodes, conns, has_audio, has_video) =
            inject_preview_subgraph(&session, "pv2", &taps, "/gw/pv2", &pipeline)
                .await
                .expect("inject should succeed for raw video");

        assert!(!has_audio && has_video);
        assert_eq!(nodes.len(), 3);

        let kinds: Vec<&str> = nodes.iter().map(|(_, k)| k.as_str()).collect();
        assert!(kinds.contains(&"video::pixel_convert"));
        // BUG (tracked in #480): bookkeeping records the unqualified kind
        // `vp9::encoder` while `add_vp9_encoder_node` actually asks the
        // engine for `video::vp9::encoder`.  The raw-audio path is
        // internally consistent (`audio::opus::encoder` on both sides) —
        // only VP9 is wrong.  This assertion pins the current behavior.
        assert!(kinds.contains(&"vp9::encoder"));
        assert!(kinds.contains(&"transport::moq::peer"));

        let ids: Vec<&str> = nodes.iter().map(|(i, _)| i.as_str()).collect();
        assert!(ids.contains(&"_preview_pv2_pixconv"));
        assert!(ids.contains(&"_preview_pv2_vp9enc"));
        assert!(ids.contains(&"_preview_pv2_peer"));

        // tap → pixconv (best-effort) → vp9enc (reliable) → peer (reliable)
        assert_eq!(conns.len(), 3);

        let tap_to_pixconv = conns
            .iter()
            .find(|(f, _, t, _, _)| f == "src" && t == "_preview_pv2_pixconv")
            .expect("tap→pixconv connection present");
        assert!(matches!(tap_to_pixconv.4, streamkit_core::control::ConnectionMode::BestEffort));

        let pixconv_to_vp9 = conns
            .iter()
            .find(|(f, _, t, _, _)| f == "_preview_pv2_pixconv" && t == "_preview_pv2_vp9enc")
            .expect("pixconv→vp9 connection present");
        assert!(matches!(pixconv_to_vp9.4, streamkit_core::control::ConnectionMode::Reliable));

        let vp9_to_peer = conns
            .iter()
            .find(|(f, _, t, _, _)| f == "_preview_pv2_vp9enc" && t == "_preview_pv2_peer")
            .expect("vp9→peer connection present");
        assert!(matches!(vp9_to_peer.4, streamkit_core::control::ConnectionMode::Reliable));

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn inject_raw_audio_creates_opus_encoder_chain() {
        let (session, _rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "src".to_string(),
            streamkit_api::Node { kind: "audio::source".to_string(), params: None, state: None },
        );
        let taps = vec![tap("src", "out", false, true, false)];

        let (nodes, conns, has_audio, has_video) =
            inject_preview_subgraph(&session, "pv3", &taps, "/gw/pv3", &pipeline)
                .await
                .expect("inject should succeed for raw audio");

        assert!(has_audio && !has_video);
        assert_eq!(nodes.len(), 2);

        let kinds: Vec<&str> = nodes.iter().map(|(_, k)| k.as_str()).collect();
        assert!(kinds.contains(&"audio::opus::encoder"));
        assert!(kinds.contains(&"transport::moq::peer"));

        assert_eq!(conns.len(), 2);
        let tap_to_opus = conns
            .iter()
            .find(|(f, _, t, _, _)| f == "src" && t == "_preview_pv3_opusenc")
            .expect("tap→opusenc connection present");
        assert!(matches!(tap_to_opus.4, streamkit_core::control::ConnectionMode::BestEffort));

        let opus_to_peer = conns
            .iter()
            .find(|(f, _, t, _, _)| f == "_preview_pv3_opusenc" && t == "_preview_pv3_peer")
            .expect("opusenc→peer connection present");
        assert!(matches!(opus_to_peer.4, streamkit_core::control::ConnectionMode::Reliable));

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn teardown_preview_removes_nodes_and_connections_from_pipeline_model() {
        let (session, _rx) = fresh_session().await;
        seed_pipeline_node(&session, "src", "audio::source").await;

        let taps = vec![tap("src", "out", false, true, false)];
        let (nodes, conns, has_audio, has_video) = {
            let pipeline = session.pipeline.lock().await.clone();
            inject_preview_subgraph(&session, "pv4", &taps, "/gw/pv4", &pipeline).await.unwrap()
        };

        {
            let mut pipeline = session.pipeline.lock().await;
            for (id, kind) in &nodes {
                pipeline.nodes.insert(
                    id.clone(),
                    streamkit_api::Node { kind: kind.clone(), params: None, state: None },
                );
            }
            for (f, fp, t, tp, mode) in &conns {
                pipeline.connections.push(streamkit_api::Connection {
                    from_node: f.clone(),
                    from_pin: fp.clone(),
                    to_node: t.clone(),
                    to_pin: tp.clone(),
                    mode: *mode,
                });
            }
        }

        let state = PreviewState {
            preview_id: "pv4".to_string(),
            tap_points: taps,
            injected_nodes: nodes.clone(),
            injected_connections: conns.clone(),
            gateway_path: "/gw/pv4".to_string(),
            has_audio,
            has_video,
            created_at: SystemTime::now(),
        };

        teardown_preview(&session, &state).await.expect("teardown should succeed");

        let pipeline = session.pipeline.lock().await;
        for (id, _) in &nodes {
            assert!(!pipeline.nodes.contains_key(id), "node {id} should be removed");
        }
        assert!(
            pipeline.connections.is_empty(),
            "all preview connections should be removed, got: {:?}",
            pipeline.connections
        );
        drop(pipeline);

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn teardown_all_previews_clears_pipeline_connections_but_keeps_preview_map() {
        // Pins TWO surprising contracts of teardown_all_previews:
        //   1. it leaves `session.previews` intact (the map is NOT drained),
        //   2. it DOES remove preview connections from `session.pipeline`.
        //
        // To make assertion (2) meaningful we must populate
        // `session.pipeline.connections` first — `inject_preview_subgraph`
        // only fills the returned bookkeeping vector, not the pipeline
        // model (the handler at start_preview_handler is what normally
        // mirrors them back).  Without this seeding the connections list
        // is empty before AND after teardown, and assertion (2) would
        // pass even if teardown_preview's pipeline cleanup branch were
        // deleted entirely.
        let (session, _rx) = fresh_session().await;
        seed_pipeline_node(&session, "src", "audio::source").await;

        let mut seeded = Vec::new();
        for i in 0..MAX_PREVIEWS_PER_SESSION {
            let preview_id = format!("p{i}");
            let taps = vec![tap("src", "out", true, true, false)];
            let pipeline = session.pipeline.lock().await.clone();
            let (nodes, conns, ha, hv) =
                inject_preview_subgraph(&session, &preview_id, &taps, "/gw", &pipeline)
                    .await
                    .unwrap();

            {
                let mut pip = session.pipeline.lock().await;
                for (node_id, kind) in &nodes {
                    pip.nodes.insert(
                        node_id.clone(),
                        streamkit_api::Node { kind: kind.clone(), params: None, state: None },
                    );
                }
                for (fn_, fp, tn, tp, mode) in &conns {
                    pip.connections.push(streamkit_api::Connection {
                        from_node: fn_.clone(),
                        from_pin: fp.clone(),
                        to_node: tn.clone(),
                        to_pin: tp.clone(),
                        mode: *mode,
                    });
                }
            }

            seeded.push((nodes.clone(), conns.clone()));

            session
                .add_preview(PreviewState {
                    preview_id: preview_id.clone(),
                    tap_points: taps,
                    injected_nodes: nodes,
                    injected_connections: conns,
                    gateway_path: format!("/gw/{preview_id}"),
                    has_audio: ha,
                    has_video: hv,
                    created_at: SystemTime::now(),
                })
                .await
                .unwrap();
        }
        assert_eq!(session.preview_count().await, MAX_PREVIEWS_PER_SESSION);

        let pip = session.pipeline.lock().await;
        assert!(
            !pip.connections.is_empty(),
            "precondition: pipeline.connections must be populated before teardown_all_previews so the cleanup assertion is meaningful"
        );
        assert!(
            seeded.iter().any(|(nodes, _)| nodes.iter().any(|(id, _)| pip.nodes.contains_key(id))),
            "precondition: at least one preview node must be present in the pipeline model"
        );
        drop(pip);

        teardown_all_previews(&session).await;

        assert_eq!(
            session.preview_count().await,
            MAX_PREVIEWS_PER_SESSION,
            "teardown_all_previews leaves the preview map intact by design"
        );

        let pipeline = session.pipeline.lock().await;
        assert!(
            pipeline.connections.is_empty(),
            "teardown_all_previews must remove every preview connection from the pipeline model, got: {:?}",
            pipeline.connections
        );
        for (nodes, _) in &seeded {
            for (id, _) in nodes {
                assert!(
                    !pipeline.nodes.contains_key(id),
                    "preview node {id} should have been removed from pipeline.nodes"
                );
            }
        }
        drop(pipeline);

        let _ = session.shutdown_and_wait().await;
    }

    /// Drain `rx` for up to `max_wait` and return every NodeAdded `kind`
    /// the engine actually emitted.  The engine emits NodeAdded only
    /// after successfully constructing the node, so this lets us
    /// distinguish between bookkeeping-only assertions and what was
    /// actually built end-to-end — catching the class of bug pinned by
    /// issue #480 (bookkeeping kind diverging from the kind asked of
    /// the engine).
    async fn drain_node_added_kinds(
        rx: &mut broadcast::Receiver<BroadcastEvent>,
        max_wait: std::time::Duration,
    ) -> Vec<String> {
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + max_wait;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(evt)) => {
                    if let streamkit_api::EventPayload::NodeAdded { kind, .. } = &evt.event.payload
                    {
                        out.push(kind.clone());
                    }
                },
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => {},
                Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => break,
            }
        }
        out
    }

    #[tokio::test]
    async fn inject_raw_video_engine_receives_video_qualified_vp9_kind() {
        // Pins what the ENGINE actually received — not just the
        // bookkeeping vector.  add_vp9_encoder_node sends
        // `video::vp9::encoder` to the engine; if a future change starts
        // sending an unqualified `vp9::encoder` (mirroring the
        // bookkeeping bug pinned by #480), the engine would fail to
        // construct the node and NodeAdded for that kind would never
        // arrive.  Same idea for opus and the moq peer.
        //
        // This complements `inject_raw_video_creates_pixconv_and_vp9_encoder`
        // (which only inspects bookkeeping) by asserting on the
        // engine-confirmed event stream.
        let (session, mut rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "src".to_string(),
            streamkit_api::Node { kind: "video::source".to_string(), params: None, state: None },
        );
        let taps = vec![tap("src", "out", false, false, true)];

        inject_preview_subgraph(&session, "engcheck", &taps, "/gw/engcheck", &pipeline)
            .await
            .expect("inject should succeed for raw video");

        let kinds = drain_node_added_kinds(&mut rx, std::time::Duration::from_secs(5)).await;

        assert!(
            kinds.contains(&"video::pixel_convert".to_string()),
            "engine never confirmed video::pixel_convert; got kinds: {kinds:?}"
        );
        assert!(
            kinds.contains(&"video::vp9::encoder".to_string()),
            "engine never confirmed video::vp9::encoder — sibling of #480? got kinds: {kinds:?}"
        );
        assert!(
            kinds.contains(&"transport::moq::peer".to_string()),
            "engine never confirmed transport::moq::peer; got kinds: {kinds:?}"
        );

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn inject_raw_audio_engine_receives_audio_qualified_opus_kind() {
        // Companion to the VP9 test above: asserts the OPUS path's
        // engine-side kind matches what add_opus_encoder_node sends
        // (`audio::opus::encoder`).  If add_opus_encoder_node ever
        // diverges from its bookkeeping (the failure mode #480 fixes
        // would have prevented in the first place), the engine never
        // confirms construction and the assertion fires.
        let (session, mut rx) = fresh_session().await;
        let mut pipeline = empty_pipeline();
        pipeline.nodes.insert(
            "src".to_string(),
            streamkit_api::Node { kind: "audio::source".to_string(), params: None, state: None },
        );
        let taps = vec![tap("src", "out", false, true, false)];

        inject_preview_subgraph(&session, "opuscheck", &taps, "/gw/opuscheck", &pipeline)
            .await
            .expect("inject should succeed for raw audio");

        let kinds = drain_node_added_kinds(&mut rx, std::time::Duration::from_secs(5)).await;

        assert!(
            kinds.contains(&"audio::opus::encoder".to_string()),
            "engine never confirmed audio::opus::encoder; got kinds: {kinds:?}"
        );
        assert!(
            kinds.contains(&"transport::moq::peer".to_string()),
            "engine never confirmed transport::moq::peer; got kinds: {kinds:?}"
        );

        let _ = session.shutdown_and_wait().await;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Panics ARE the assertions
mod handler_tests {
    use super::*;
    use crate::config::Config;
    use crate::permissions::Permissions;
    use crate::session::{PreviewState, Session, TapPoint};
    use axum::body::{to_bytes, Body};
    use axum::http::{Method, Request, StatusCode};
    use axum::routing::{delete, post};
    use axum::Router;
    use serde_json::json;
    use std::time::SystemTime;
    use tower::ServiceExt;

    fn admin_permissions() -> Permissions {
        let mut p = Permissions::admin();
        p.list_sessions = true;
        p.modify_sessions = true;
        p.access_all_sessions = true;
        p
    }

    // Same env-leak guard as the plugins handler_tests module —
    // see comment there.
    static ENV_SETUP: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    fn ensure_clean_env() {
        ENV_SETUP.get_or_init(|| std::env::remove_var("SK_ROLE"));
    }

    /// `(state, _tmp)` — the caller must keep the TempDir alive for the
    /// lifetime of the test so the temp `.plugins/` directory survives
    /// every request.  Without this override the plugin manager init in
    /// `create_app_state` calls `create_dir_all(".plugins/...")` against
    /// the test process's CWD, leaving a `.plugins/` directory at the
    /// workspace root and racing with parallel tests that share CWD.
    fn make_admin_state() -> (Arc<AppState>, tempfile::TempDir) {
        ensure_clean_env();
        let tmp = tempfile::TempDir::new().unwrap();
        let mut cfg = Config::default();
        cfg.plugins.directory = tmp.path().to_string_lossy().into_owned();
        cfg.permissions.default_role = "preview-admin".to_string();
        cfg.permissions.roles.insert("preview-admin".to_string(), admin_permissions());
        (crate::server::create_app_state(cfg, None), tmp)
    }

    async fn install_session(state: &Arc<AppState>, name: &str) -> Session {
        let event_tx = state.event_tx.clone();
        let session =
            Session::create(&state.engine, &state.config, Some(name.to_string()), event_tx, None)
                .await
                .expect("Session::create succeeds");
        state.session_manager.lock().await.add_session(session.clone()).expect("insert session");
        session
    }

    fn build_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route(
                "/api/v1/sessions/{session_id}/preview",
                post(start_preview_handler).get(list_previews_handler),
            )
            .route(
                "/api/v1/sessions/{session_id}/preview/{preview_id}",
                delete(stop_preview_handler),
            )
            .with_state(state)
    }

    async fn read_body(resp: axum::response::Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn post_json(uri: &str, body: &serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn start_preview_returns_404_for_unknown_session() {
        // Precondition: this module is gated on `feature = "moq"`
        // (server/mod.rs), and `create_app_state` under that feature
        // always sets `app_state.moq_gateway = Some(...)`.  The handler's
        // earlier `moq_gateway.is_none()` 503 branch is therefore
        // unreachable here — we always hit the session-lookup 404.
        // If the gateway ever becomes opt-in, this assertion will need a
        // setup step that explicitly enables it.
        let (state, _tmp) = make_admin_state();
        let router = build_router(state);

        let req = post_json("/api/v1/sessions/missing/preview", &json!({}));
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("missing"), "body: {body}");
    }

    #[tokio::test]
    async fn start_preview_returns_400_when_tap_node_not_in_pipeline() {
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;
        let router = build_router(Arc::clone(&state));

        let req = post_json(
            "/api/v1/sessions/alpha/preview",
            &json!({ "tap_node": "ghost", "tap_pin": "out" }),
        );
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Tap node 'ghost' not found"), "body: {body}");

        // The error message is shared between the handler's pre-check
        // (preview.rs:751-756) and inject_preview_subgraph's own check.
        // A failure-mode-distinguishing assertion: after a pre-check
        // rejection, NO preview nodes can have been added to the
        // session pipeline.  If a future refactor accidentally lets
        // inject_preview_subgraph run before the handler short-circuits,
        // it would push partial state through the engine and that
        // state would be observable on session.pipeline.
        let pip = session.pipeline.lock().await;
        assert!(
            !pip.nodes.keys().any(|k| k.starts_with("_preview_")),
            "pre-check failure must not leak any _preview_ nodes into the pipeline, got: {:?}",
            pip.nodes.keys().collect::<Vec<_>>()
        );
        drop(pip);

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn start_preview_returns_400_when_tap_pin_not_on_node() {
        // Pins the pin-validation pre-check at preview.rs:761-765,
        // which produces a UNIQUE error message that distinguishes it
        // from the tap_node and inject_preview_subgraph checks.
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;

        // Seed a real node whose kind is registered with the engine, so
        // the registry lookup finds a definition and the pin check
        // actually runs (instead of warning + skipping).
        // `video::colorbars` is registered by streamkit-nodes and has
        // a single `out` output pin (crates/nodes/src/video/colorbars.rs).
        {
            let mut pip = session.pipeline.lock().await;
            pip.nodes.insert(
                "src".to_string(),
                streamkit_api::Node {
                    kind: "video::colorbars".to_string(),
                    params: None,
                    state: None,
                },
            );
        }

        let router = build_router(Arc::clone(&state));
        let req = post_json(
            "/api/v1/sessions/alpha/preview",
            &json!({ "tap_node": "src", "tap_pin": "this-pin-does-not-exist" }),
        );
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.contains("Pin 'this-pin-does-not-exist' not found on node 'src'"),
            "body: {body}"
        );

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn start_preview_returns_400_when_tap_pin_provided_without_tap_node() {
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;
        let router = build_router(Arc::clone(&state));

        let req = post_json("/api/v1/sessions/alpha/preview", &json!({ "tap_pin": "out" }));
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("tap_pin requires tap_node"), "body: {body}");

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn start_preview_returns_409_when_preview_limit_reached() {
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;

        for i in 0..MAX_PREVIEWS_PER_SESSION {
            let preview_id = format!("seed{i}");
            session
                .add_preview(PreviewState {
                    preview_id: preview_id.clone(),
                    tap_points: vec![TapPoint {
                        node: "src".into(),
                        pin: "out".into(),
                        is_encoded: true,
                        is_audio: true,
                        is_video: false,
                    }],
                    injected_nodes: vec![],
                    injected_connections: vec![],
                    gateway_path: format!("/gw/{preview_id}"),
                    has_audio: true,
                    has_video: false,
                    created_at: SystemTime::now(),
                })
                .await
                .unwrap();
        }

        let router = build_router(Arc::clone(&state));
        let req = post_json("/api/v1/sessions/alpha/preview", &json!({}));
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(body.contains("Maximum"), "body: {body}");
        assert!(body.contains(&MAX_PREVIEWS_PER_SESSION.to_string()), "body: {body}");

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn list_previews_returns_404_for_unknown_session() {
        let (state, _tmp) = make_admin_state();
        let router = build_router(state);

        let req =
            Request::builder().uri("/api/v1/sessions/missing/preview").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, _) = read_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_previews_returns_seeded_previews_in_order() {
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;

        let earlier = SystemTime::now();
        let later = earlier + std::time::Duration::from_secs(10);

        session
            .add_preview(PreviewState {
                preview_id: "old".into(),
                tap_points: vec![TapPoint {
                    node: "src".into(),
                    pin: "out".into(),
                    is_encoded: true,
                    is_audio: true,
                    is_video: false,
                }],
                injected_nodes: vec![],
                injected_connections: vec![],
                gateway_path: "/gw/old".into(),
                has_audio: true,
                has_video: false,
                created_at: earlier,
            })
            .await
            .unwrap();
        session
            .add_preview(PreviewState {
                preview_id: "new".into(),
                tap_points: vec![TapPoint {
                    node: "src".into(),
                    pin: "out".into(),
                    is_encoded: true,
                    is_audio: false,
                    is_video: true,
                }],
                injected_nodes: vec![],
                injected_connections: vec![],
                gateway_path: "/gw/new".into(),
                has_audio: false,
                has_video: true,
                created_at: later,
            })
            .await
            .unwrap();

        let router = build_router(Arc::clone(&state));
        let req =
            Request::builder().uri("/api/v1/sessions/alpha/preview").body(Body::empty()).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::OK);

        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        let items = parsed.as_array().expect("array");
        assert_eq!(items.len(), 2);
        let ids: Vec<&str> = items.iter().map(|p| p["preview_id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"old") && ids.contains(&"new"));

        // Each entry exposes the documented per-tap media classification.
        for item in items {
            let taps = item["tap_points"].as_array().expect("tap_points array");
            for t in taps {
                let media = t["media"].as_str().unwrap();
                assert!(matches!(media, "audio" | "video" | "audio+video"));
            }
        }

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn stop_preview_returns_404_for_unknown_session() {
        let (state, _tmp) = make_admin_state();
        let router = build_router(state);

        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/sessions/missing/preview/anything")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        // Pin the source of the 404 to the session-lookup branch in
        // stop_preview_handler; an unrelated 404 (route fall-through,
        // future ownership check that short-circuits to NOT_FOUND) would
        // pass a status-only assertion but fail this one.
        assert!(body.contains("missing"), "body: {body}");
    }

    #[tokio::test]
    async fn stop_preview_returns_404_for_unknown_preview_id() {
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;
        let router = build_router(Arc::clone(&state));

        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/sessions/alpha/preview/ghost")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("ghost"), "body: {body}");

        let _ = session.shutdown_and_wait().await;
    }

    #[tokio::test]
    async fn stop_preview_removes_preview_and_clears_pipeline_model() {
        let (state, _tmp) = make_admin_state();
        let session = install_session(&state, "alpha").await;

        // Seed an injected node + connection so teardown has something
        // observable to remove from the pipeline model.
        {
            let mut pipeline = session.pipeline.lock().await;
            pipeline.nodes.insert(
                "_preview_pv_peer".into(),
                streamkit_api::Node {
                    kind: "transport::moq::peer".into(),
                    params: None,
                    state: None,
                },
            );
            pipeline.connections.push(streamkit_api::Connection {
                from_node: "src".into(),
                from_pin: "out".into(),
                to_node: "_preview_pv_peer".into(),
                to_pin: "in".into(),
                mode: streamkit_core::control::ConnectionMode::BestEffort,
            });
        }

        session
            .add_preview(PreviewState {
                preview_id: "pv".into(),
                tap_points: vec![TapPoint {
                    node: "src".into(),
                    pin: "out".into(),
                    is_encoded: true,
                    is_audio: true,
                    is_video: false,
                }],
                injected_nodes: vec![("_preview_pv_peer".into(), "transport::moq::peer".into())],
                injected_connections: vec![(
                    "src".into(),
                    "out".into(),
                    "_preview_pv_peer".into(),
                    "in".into(),
                    streamkit_core::control::ConnectionMode::BestEffort,
                )],
                gateway_path: "/gw/pv".into(),
                has_audio: true,
                has_video: false,
                created_at: SystemTime::now(),
            })
            .await
            .unwrap();

        let router = build_router(Arc::clone(&state));
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/sessions/alpha/preview/pv")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["preview_id"], "pv");

        assert!(session.remove_preview("pv").await.is_none());

        let pipeline = session.pipeline.lock().await;
        assert!(!pipeline.nodes.contains_key("_preview_pv_peer"));
        assert!(pipeline.connections.is_empty());
        drop(pipeline);

        let _ = session.shutdown_and_wait().await;
    }
}
