// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::{
    CaptureSource, ClientSection, ControlType, InputType, PublishConfig, PublishTrackConfig,
    TrackKind,
};
use crate::EngineMode;

/// A single lint warning produced by [`lint_client_section`].
#[derive(Debug, Clone)]
pub struct ClientLintWarning {
    /// Machine-readable rule identifier (e.g. `"mode-mismatch"`).
    pub rule: &'static str,
    /// Human-readable description of the problem.
    pub message: String,
}

/// Validates the `client` section against the compiled pipeline, returning
/// any semantic warnings.
///
/// This is a *lint* pass — it never prevents compilation, but surfaces
/// likely authoring mistakes so tooling (CLI, editor integrations) can
/// flag them.
///
/// # Rules
///
///  1. **`mode-mismatch-dynamic`** — Dynamic pipeline declares oneshot-only
///     fields (`input` / `output`).
///  2. **`mode-mismatch-oneshot`** — Oneshot pipeline declares dynamic-only
///     fields (`publish` / `watch` / `gateway_path` / `relay_url` / `controls`).
///  3. **`missing-gateway`** — Dynamic pipeline has `publish` or `watch`
///     but no `gateway_path` or `relay_url`.
///  4. **`publish-no-media`** — `publish` block sets both `audio` and
///     `video` to false.
///  5. **`watch-no-media`** — `watch` block sets both `audio` and `video`
///     to false.
///  6. **`input-none-with-accept`** — `input.type` is `none` but `accept`
///     is set (accept is meaningless without a file picker).
///  7. **`input-trigger-with-accept`** — `input.type` is `trigger` but
///     `accept` is set.
///  8. **`field-hints-no-input`** — `field_hints` is present but
///     `input.type` is `none`.
///  9. **`asset-tags-no-input`** — `asset_tags` is present but
///     `input.type` is `none` or `text`.
/// 10. **`text-no-placeholder`** — `input.type` is `text` but no
///     `placeholder` is provided (best-practice hint).
/// 11. **`empty-broadcast`** — `publish.broadcast` or `watch.broadcast`
///     is an empty string.
/// 12. **`duplicate-broadcast`** — `publish.broadcast` equals
///     `watch.broadcast` (would cause a loop).
/// 13. **`empty-tracks`** — `publish.tracks` is an empty array.
/// 14. **`kind-source-mismatch`** — Track kind and source are incompatible
///     (e.g. `kind: audio` with `source: camera`).
/// 15. **`duplicate-source`** — Multiple tracks use the same kind and capture
///     source combination.
/// 16. **`empty-track-broadcast`** — A track-level `broadcast` override is
///     an empty string.
pub fn lint_client_section(client: &ClientSection, mode: EngineMode) -> Vec<ClientLintWarning> {
    let mut warnings = Vec::new();

    let has_dynamic_fields = client.gateway_path.is_some()
        || client.relay_url.is_some()
        || client.publish.is_some()
        || client.watch.is_some()
        || client.controls.is_some();

    let has_oneshot_fields = client.input.is_some() || client.output.is_some();

    if mode == EngineMode::Dynamic && has_oneshot_fields {
        warnings.push(ClientLintWarning {
            rule: "mode-mismatch-dynamic",
            message: "Dynamic pipeline declares `input` or `output` — these are oneshot-only \
                      fields and will be ignored."
                .into(),
        });
    }

    if mode == EngineMode::OneShot && has_dynamic_fields {
        warnings.push(ClientLintWarning {
            rule: "mode-mismatch-oneshot",
            message: "Oneshot pipeline declares `publish`, `watch`, `gateway_path`, \
                      `relay_url`, or `controls` — these are dynamic-only fields and \
                      will be ignored."
                .into(),
        });
    }

    let watch_needs_gateway = client.watch.as_ref().is_some_and(|w| w.broadcast.is_some());
    if (client.publish.is_some() || watch_needs_gateway)
        && client.gateway_path.is_none()
        && client.relay_url.is_none()
    {
        warnings.push(ClientLintWarning {
            rule: "missing-gateway",
            message: "Pipeline has `publish` or `watch` but no `gateway_path` or `relay_url` — \
                      the browser won't know where to connect."
                .into(),
        });
    }

    if let Some(ref publish) = client.publish {
        lint_publish_section(&mut warnings, publish);
    }

    if let Some(ref watch) = client.watch {
        if !watch.audio && !watch.video {
            warnings.push(ClientLintWarning {
                rule: "watch-no-media",
                message: "watch block sets both `audio` and `video` to false — nothing will be \
                          received by the browser."
                    .into(),
            });
        }

        if watch.broadcast.as_deref() == Some("") {
            warnings.push(ClientLintWarning {
                rule: "empty-broadcast",
                message: "watch.broadcast is an empty string.".into(),
            });
        }
    }

    if let (Some(ref publish), Some(ref watch)) = (&client.publish, &client.watch) {
        if let Some(ref watch_bc) = watch.broadcast {
            if !watch_bc.is_empty() {
                let mut publish_broadcasts: Vec<&str> = Vec::new();
                if !publish.broadcast.is_empty() {
                    publish_broadcasts.push(&publish.broadcast);
                }
                for track in &publish.tracks {
                    if let Some(ref bc) = track.broadcast {
                        if !bc.is_empty() {
                            publish_broadcasts.push(bc);
                        }
                    }
                }
                publish_broadcasts.sort_unstable();
                publish_broadcasts.dedup();

                for bc in publish_broadcasts {
                    if bc == watch_bc {
                        warnings.push(ClientLintWarning {
                            rule: "duplicate-broadcast",
                            message: format!(
                                "Publish broadcast '{bc}' matches watch.broadcast '{watch_bc}' \
                                 — this would cause a feedback loop.",
                            ),
                        });
                    }
                }
            }
        }
    }

    if let Some(ref input) = client.input {
        if matches!(input.input_type, InputType::None) && input.accept.is_some() {
            warnings.push(ClientLintWarning {
                rule: "input-none-with-accept",
                message: "input.type is `none` but `accept` is set — accept is meaningless \
                          without a file picker."
                    .into(),
            });
        }

        if matches!(input.input_type, InputType::Trigger) && input.accept.is_some() {
            warnings.push(ClientLintWarning {
                rule: "input-trigger-with-accept",
                message: "input.type is `trigger` but `accept` is set — accept is meaningless \
                          for trigger inputs."
                    .into(),
            });
        }

        if matches!(input.input_type, InputType::None)
            && input.field_hints.as_ref().is_some_and(|h| !h.is_empty())
        {
            warnings.push(ClientLintWarning {
                rule: "field-hints-no-input",
                message: "field_hints is present but input.type is `none` — hints are unused \
                          without an input."
                    .into(),
            });
        }

        if matches!(input.input_type, InputType::None | InputType::Text)
            && input.asset_tags.as_ref().is_some_and(|t| !t.is_empty())
        {
            warnings.push(ClientLintWarning {
                rule: "asset-tags-no-input",
                message: "asset_tags is present but input.type is `none` or `text` — tags are \
                          only useful for file_upload inputs."
                    .into(),
            });
        }

        if matches!(input.input_type, InputType::Text) && input.placeholder.is_none() {
            warnings.push(ClientLintWarning {
                rule: "text-no-placeholder",
                message: "input.type is `text` but no `placeholder` is provided — consider \
                          adding one for a better UX."
                    .into(),
            });
        }
    }

    warnings
}

fn lint_publish_section(warnings: &mut Vec<ClientLintWarning>, publish: &PublishConfig) {
    if publish.tracks.is_empty() {
        warnings.push(ClientLintWarning {
            rule: "empty-tracks",
            message: "publish.tracks is empty — nothing will be captured from the browser.".into(),
        });
    }

    if publish.broadcast.is_empty() {
        warnings.push(ClientLintWarning {
            rule: "empty-broadcast",
            message: "publish.broadcast is an empty string.".into(),
        });
    }

    for track in &publish.tracks {
        lint_publish_track(warnings, track);
    }

    {
        let mut seen_per_broadcast: std::collections::HashMap<
            &str,
            Vec<(TrackKind, CaptureSource)>,
        > = std::collections::HashMap::new();
        for track in &publish.tracks {
            let effective_bc = track.broadcast.as_deref().unwrap_or(&publish.broadcast);
            let seen = seen_per_broadcast.entry(effective_bc).or_default();
            let key = (track.kind, track.source);
            if seen.contains(&key) {
                warnings.push(ClientLintWarning {
                    rule: "duplicate-source",
                    message: format!(
                        "Multiple tracks in broadcast '{}' use the same kind `{}` and capture \
                         source `{}`.",
                        effective_bc, track.kind, track.source
                    ),
                });
            } else {
                seen.push(key);
            }
        }
    }
}

fn lint_publish_track(warnings: &mut Vec<ClientLintWarning>, track: &PublishTrackConfig) {
    let mismatch = matches!(
        (track.kind, track.source),
        (TrackKind::Audio, CaptureSource::Camera | CaptureSource::Screen)
            | (TrackKind::Video, CaptureSource::Microphone)
    );
    if mismatch {
        warnings.push(ClientLintWarning {
            rule: "kind-source-mismatch",
            message: format!(
                "Track has kind `{}` with source `{}` — these are incompatible.",
                track.kind, track.source
            ),
        });
    }

    if let Some(ref bc) = track.broadcast {
        if bc.is_empty() {
            warnings.push(ClientLintWarning {
                rule: "empty-track-broadcast",
                message: "A track-level `broadcast` override is an empty string.".into(),
            });
        }
    }

    if track.kind == TrackKind::Audio && (track.width.is_some() || track.height.is_some()) {
        warnings.push(ClientLintWarning {
            rule: "dimensions-on-audio",
            message: format!(
                "Audio track (source=`{}`) sets width/height — these fields \
                 only apply to video tracks and will be ignored.",
                track.source
            ),
        });
    }

    if track.kind == TrackKind::Video && (track.width.is_some() != track.height.is_some()) {
        let has = if track.width.is_some() { "width" } else { "height" };
        let missing = if track.width.is_some() { "height" } else { "width" };
        warnings.push(ClientLintWarning {
            rule: "partial-dimensions",
            message: format!(
                "Video track (source=`{}`) sets {has} but not {missing} — \
                 both should be specified together for correct maxPixels computation.",
                track.source,
            ),
        });
    }

    if let Some(ref codec) = track.codec {
        let recognized = match track.kind {
            TrackKind::Video => ["vp9", "av1", "h264"].contains(&codec.as_str()),
            TrackKind::Audio => ["opus", "aac"].contains(&codec.as_str()),
        };
        if !recognized {
            let supported = match track.kind {
                TrackKind::Video => "vp9, av1, h264",
                TrackKind::Audio => "opus, aac",
            };
            warnings.push(ClientLintWarning {
                rule: "unrecognized-codec",
                message: format!(
                    "Track (kind=`{}`, source=`{}`) has unrecognized codec \
                     `{codec}` — supported: {supported}. Unrecognized codecs \
                     will hard-fail at encoder init.",
                    track.kind, track.source
                ),
            });
        }
    }

    if track.kind == TrackKind::Video && (track.width == Some(0) || track.height == Some(0)) {
        warnings.push(ClientLintWarning {
            rule: "zero-dimension",
            message: format!(
                "Track (kind=`{}`, source=`{}`) has zero width or height — \
                 this will produce degenerate encoder output.",
                track.kind, track.source
            ),
        });
    }
    if track.max_bitrate == Some(0) {
        warnings.push(ClientLintWarning {
            rule: "zero-bitrate",
            message: format!(
                "Track (kind=`{}`, source=`{}`) has max_bitrate: 0 — \
                 this will likely cause the encoder to fail.",
                track.kind, track.source
            ),
        });
    }

    if track.kind == TrackKind::Audio && track.max_bitrate.is_some() {
        warnings.push(ClientLintWarning {
            rule: "bitrate-on-audio",
            message: format!(
                "Audio track (source=`{}`) sets max_bitrate — audio bitrate \
                 is parsed but not yet wired to the audio encoder and will \
                 have no effect.",
                track.source
            ),
        });
    }
}

/// A lightweight view of a pipeline's nodes used by
/// [`lint_client_against_nodes`] for cross-validation.
///
/// Callers construct this from either `UserPipeline::Dag` nodes or
/// `UserPipeline::Steps` steps.
pub struct NodeInfo<'a> {
    /// The user-facing node ID (the key in the `nodes:` map).
    pub name: &'a str,
    pub kind: &'a str,
    pub params: Option<&'a serde_json::Value>,
}

/// Cross-validates the `client` section against the pipeline's node graph.
///
/// This is a second lint layer that complements [`lint_client_section`]
/// (which checks `client` in isolation).  The rules here require knowledge
/// of which nodes exist and their params.
///
/// # Rules
///
/// 13. **`input-requires-http-input`** — `input.type` is `file_upload`,
///     `text`, or `trigger` but no `streamkit::http_input` node exists.
/// 14. **`input-none-has-http-input`** — `input.type` is `none` but an
///     `streamkit::http_input` node exists (should be `trigger`).
/// 15. **`field-hint-unknown-field`** — `field_hints` references a field
///     name not found in any `streamkit::http_input` node's `fields` param.
/// 16. **`publish-no-transport`** — `publish` is declared but no MoQ
///     transport node (`transport::moq::peer` or
///     `transport::moq::subscriber`) exists.
/// 17. **`watch-no-transport`** — `watch` is declared but no MoQ transport
///     node (`transport::moq::peer` or `transport::moq::publisher`) exists.
/// 18. **`gateway-path-mismatch`** — `client.gateway_path` does not match
///     the `gateway_path` param on a `transport::moq::peer` node.
/// 19. **`relay-url-mismatch`** — `client.relay_url` does not match the
///     `url` param on a `transport::moq::publisher` or
///     `transport::moq::subscriber` node.
/// 20. **`broadcast-mismatch`** — `publish.broadcast` or `watch.broadcast`
///     does not match any broadcast name configured on MoQ transport nodes.
/// 21. **`control-unknown-node`** — a `controls` entry targets a `node` that
///     does not exist in the pipeline's `nodes` map.
/// 22. **`control-number-no-bounds`** — a `number` control is missing `min`
///     and/or `max`, so the slider will lack proper bounds.
/// 23. **`control-select-no-options`** — a `select` control has no `options`
///     array, so the dropdown will be empty.
pub fn lint_client_against_nodes(
    client: &ClientSection,
    _mode: EngineMode,
    nodes: &[NodeInfo<'_>],
) -> Vec<ClientLintWarning> {
    let mut warnings = Vec::new();

    let has_http_input = nodes.iter().any(|n| n.kind == "streamkit::http_input");
    let has_moq_peer = nodes.iter().any(|n| n.kind == "transport::moq::peer");
    let has_moq_subscriber = nodes.iter().any(|n| n.kind == "transport::moq::subscriber");
    let has_moq_publisher = nodes.iter().any(|n| n.kind == "transport::moq::publisher");

    if let Some(ref input) = client.input {
        let needs_http_input = matches!(
            input.input_type,
            InputType::FileUpload | InputType::Text | InputType::Trigger
        );
        if needs_http_input && !has_http_input {
            warnings.push(ClientLintWarning {
                rule: "input-requires-http-input",
                message: format!(
                    "input.type is `{}` but no `streamkit::http_input` node exists.",
                    match input.input_type {
                        InputType::FileUpload => "file_upload",
                        InputType::Text => "text",
                        InputType::Trigger => "trigger",
                        InputType::None => "none",
                    }
                ),
            });
        }

        if matches!(input.input_type, InputType::None) && has_http_input {
            warnings.push(ClientLintWarning {
                rule: "input-none-has-http-input",
                message: "input.type is `none` but a `streamkit::http_input` node exists — \
                          consider using `trigger` instead."
                    .into(),
            });
        }

        if let Some(ref hints) = input.field_hints {
            let mut declared_fields: Vec<String> = Vec::new();
            for node in nodes.iter().filter(|n| n.kind == "streamkit::http_input") {
                if let Some(params) = node.params {
                    if let Some(name) =
                        params.get("field").and_then(|f| f.get("name")).and_then(|n| n.as_str())
                    {
                        declared_fields.push(name.to_string());
                    }
                    if let Some(fields_arr) = params.get("fields").and_then(|f| f.as_array()) {
                        for f in fields_arr {
                            if let Some(name) = f.get("name").and_then(|n| n.as_str()) {
                                declared_fields.push(name.to_string());
                            }
                        }
                    }
                }
                if (node.params.is_none()
                    || node
                        .params
                        .is_none_or(|p| p.get("field").is_none() && p.get("fields").is_none()))
                    && !declared_fields.contains(&"media".to_string())
                {
                    declared_fields.push("media".to_string());
                }
            }

            if !declared_fields.is_empty() {
                for hint_name in hints.keys() {
                    if !declared_fields.iter().any(|f| f == hint_name) {
                        warnings.push(ClientLintWarning {
                            rule: "field-hint-unknown-field",
                            message: format!(
                                "field_hints references `{hint_name}` but no `streamkit::http_input` \
                                 node declares a field with that name. Known fields: {}.",
                                declared_fields.join(", ")
                            ),
                        });
                    }
                }
            }
        }
    }

    if client.publish.is_some() && !has_moq_peer && !has_moq_subscriber {
        warnings.push(ClientLintWarning {
            rule: "publish-no-transport",
            message: "client declares `publish` but no `transport::moq::peer` or \
                      `transport::moq::subscriber` node exists."
                .into(),
        });
    }

    let watch_needs_moq = client.watch.as_ref().is_some_and(|w| w.broadcast.is_some());
    if watch_needs_moq && !has_moq_peer && !has_moq_publisher {
        warnings.push(ClientLintWarning {
            rule: "watch-no-transport",
            message: "client declares `watch` but no `transport::moq::peer` or \
                      `transport::moq::publisher` node exists."
                .into(),
        });
    }

    if let Some(ref client_gw) = client.gateway_path {
        let peer_gateway_paths: Vec<&str> = nodes
            .iter()
            .filter(|n| n.kind == "transport::moq::peer")
            .filter_map(|n| n.params.and_then(|p| p.get("gateway_path")).and_then(|v| v.as_str()))
            .collect();

        if !peer_gateway_paths.is_empty() && !peer_gateway_paths.iter().any(|gw| gw == client_gw) {
            warnings.push(ClientLintWarning {
                rule: "gateway-path-mismatch",
                message: format!(
                    "client.gateway_path is `{client_gw}` but moq::peer node(s) declare: {}.",
                    peer_gateway_paths.join(", ")
                ),
            });
        }
    }

    if let Some(ref client_url) = client.relay_url {
        let node_urls: Vec<&str> = nodes
            .iter()
            .filter(|n| {
                n.kind == "transport::moq::publisher" || n.kind == "transport::moq::subscriber"
            })
            .filter_map(|n| n.params.and_then(|p| p.get("url")).and_then(|v| v.as_str()))
            .collect();

        if !node_urls.is_empty() && !node_urls.iter().any(|u| u == client_url) {
            warnings.push(ClientLintWarning {
                rule: "relay-url-mismatch",
                message: format!(
                    "client.relay_url is `{client_url}` but transport node(s) declare: {}.",
                    node_urls.join(", ")
                ),
            });
        }
    }

    let mut node_broadcasts: Vec<&str> = Vec::new();
    for node in nodes {
        if let Some(params) = node.params {
            match node.kind {
                "transport::moq::peer" => {
                    if let Some(arr) = params.get("input_broadcasts").and_then(|v| v.as_array()) {
                        for item in arr {
                            if let Some(b) = item.as_str() {
                                node_broadcasts.push(b);
                            }
                        }
                    }
                    if let Some(b) = params.get("output_broadcast").and_then(|v| v.as_str()) {
                        node_broadcasts.push(b);
                    }
                },
                "transport::moq::publisher" | "transport::moq::subscriber" => {
                    if let Some(b) = params.get("broadcast").and_then(|v| v.as_str()) {
                        node_broadcasts.push(b);
                    }
                },
                _ => {},
            }
        }
    }

    if !node_broadcasts.is_empty() {
        if let Some(ref publish) = client.publish {
            let mut publish_broadcasts: Vec<&str> = vec![publish.broadcast.as_str()];
            for track in &publish.tracks {
                if let Some(ref bc) = track.broadcast {
                    if !publish_broadcasts.contains(&bc.as_str()) {
                        publish_broadcasts.push(bc.as_str());
                    }
                }
            }

            for bc in &publish_broadcasts {
                if !bc.is_empty() && !node_broadcasts.iter().any(|b| b == bc) {
                    warnings.push(ClientLintWarning {
                        rule: "broadcast-mismatch",
                        message: format!(
                            "publish broadcast `{bc}` does not match any MoQ transport node \
                             broadcast name. Node broadcasts: {}.",
                            node_broadcasts.join(", ")
                        ),
                    });
                }
            }
        }
        if let Some(ref watch) = client.watch {
            if let Some(ref watch_bc) = watch.broadcast {
                if !watch_bc.is_empty() && !node_broadcasts.iter().any(|b| b == watch_bc) {
                    warnings.push(ClientLintWarning {
                        rule: "broadcast-mismatch",
                        message: format!(
                            "watch.broadcast is `{}` but no MoQ transport node declares \
                             that broadcast name. Node broadcasts: {}.",
                            watch_bc,
                            node_broadcasts.join(", ")
                        ),
                    });
                }
            }
        }
    }

    if let Some(ref controls) = client.controls {
        let node_names: Vec<&str> = nodes.iter().map(|n| n.name).collect();

        for control in controls {
            if !node_names.iter().any(|n| *n == control.node) {
                warnings.push(ClientLintWarning {
                    rule: "control-unknown-node",
                    message: format!(
                        "control `{}` targets node `{}` which does not exist in the pipeline. \
                         Known nodes: {}.",
                        control.label,
                        control.node,
                        if node_names.is_empty() {
                            "(none)".to_string()
                        } else {
                            node_names.join(", ")
                        }
                    ),
                });
            }

            if matches!(control.control_type, ControlType::Number)
                && (control.min.is_none() || control.max.is_none())
            {
                warnings.push(ClientLintWarning {
                    rule: "control-number-no-bounds",
                    message: format!(
                        "control `{}` is type `number` but is missing {} — the slider \
                         will not have proper bounds.",
                        control.label,
                        match (control.min.is_none(), control.max.is_none()) {
                            (true, true) => "`min` and `max`",
                            (true, false) => "`min`",
                            (false, true) => "`max`",
                            _ => unreachable!(),
                        }
                    ),
                });
            }

            if matches!(control.control_type, ControlType::Select)
                && control.options.as_ref().is_none_or(|opts| opts.is_empty())
            {
                warnings.push(ClientLintWarning {
                    rule: "control-select-no-options",
                    message: format!(
                        "control `{}` is type `select` but has no `options` — the \
                         dropdown will be empty.",
                        control.label,
                    ),
                });
            }
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::{
        ControlConfig, FieldHint, InputConfig, OutputConfig, OutputType, WatchConfig,
    };
    use indexmap::IndexMap;

    fn dynamic_with_gateway() -> ClientSection {
        ClientSection { gateway_path: Some("/gw".into()), ..ClientSection::default() }
    }

    fn audio_track(broadcast: Option<&str>) -> PublishTrackConfig {
        PublishTrackConfig {
            kind: TrackKind::Audio,
            source: CaptureSource::Microphone,
            broadcast: broadcast.map(str::to_string),
            width: None,
            height: None,
            codec: None,
            max_bitrate: None,
        }
    }

    fn video_track(source: CaptureSource) -> PublishTrackConfig {
        PublishTrackConfig {
            kind: TrackKind::Video,
            source,
            broadcast: None,
            width: None,
            height: None,
            codec: None,
            max_bitrate: None,
        }
    }

    fn publish_one_audio(broadcast: &str) -> PublishConfig {
        PublishConfig { broadcast: broadcast.into(), tracks: vec![audio_track(None)] }
    }

    fn watch_audio_only() -> WatchConfig {
        WatchConfig { broadcast: None, mse_path: None, audio: true, video: false }
    }

    fn ruled(client: &ClientSection, mode: EngineMode, rule: &str) -> bool {
        lint_client_section(client, mode).iter().any(|w| w.rule == rule)
    }

    struct LintCase {
        rule: &'static str,
        mode: EngineMode,
        build: fn() -> ClientSection,
    }

    fn case_mode_mismatch_dynamic() -> ClientSection {
        ClientSection {
            output: Some(OutputConfig { output_type: OutputType::Audio }),
            ..dynamic_with_gateway()
        }
    }

    fn case_mode_mismatch_oneshot() -> ClientSection {
        ClientSection {
            publish: Some(publish_one_audio("bcast")),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    fn case_missing_gateway() -> ClientSection {
        ClientSection { publish: Some(publish_one_audio("bcast")), ..ClientSection::default() }
    }

    fn case_watch_no_media() -> ClientSection {
        ClientSection {
            watch: Some(WatchConfig {
                broadcast: None,
                mse_path: None,
                audio: false,
                video: false,
            }),
            ..ClientSection::default()
        }
    }

    fn case_input_none_with_accept() -> ClientSection {
        ClientSection {
            input: Some(InputConfig {
                input_type: InputType::None,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            ..ClientSection::default()
        }
    }

    fn case_input_trigger_with_accept() -> ClientSection {
        ClientSection {
            input: Some(InputConfig {
                input_type: InputType::Trigger,
                accept: Some("audio/*".into()),
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            ..ClientSection::default()
        }
    }

    fn case_field_hints_no_input() -> ClientSection {
        let mut hints = IndexMap::new();
        hints.insert(
            "media".to_string(),
            FieldHint { field_type: None, accept: None, placeholder: None },
        );
        ClientSection {
            input: Some(InputConfig {
                input_type: InputType::None,
                accept: None,
                asset_tags: None,
                placeholder: None,
                field_hints: Some(hints),
            }),
            ..ClientSection::default()
        }
    }

    fn case_asset_tags_no_input() -> ClientSection {
        ClientSection {
            input: Some(InputConfig {
                input_type: InputType::None,
                accept: None,
                asset_tags: Some(vec!["speech".into()]),
                placeholder: None,
                field_hints: None,
            }),
            ..ClientSection::default()
        }
    }

    fn case_text_no_placeholder() -> ClientSection {
        ClientSection {
            input: Some(InputConfig {
                input_type: InputType::Text,
                accept: None,
                asset_tags: None,
                placeholder: None,
                field_hints: None,
            }),
            ..ClientSection::default()
        }
    }

    fn case_empty_broadcast() -> ClientSection {
        ClientSection {
            publish: Some(publish_one_audio("")),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    fn case_duplicate_broadcast() -> ClientSection {
        ClientSection {
            publish: Some(publish_one_audio("shared")),
            watch: Some(WatchConfig {
                broadcast: Some("shared".into()),
                mse_path: None,
                audio: true,
                video: false,
            }),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    fn case_empty_tracks() -> ClientSection {
        ClientSection {
            publish: Some(PublishConfig { broadcast: "bcast".into(), tracks: vec![] }),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    fn case_kind_source_mismatch() -> ClientSection {
        ClientSection {
            publish: Some(PublishConfig {
                broadcast: "bcast".into(),
                tracks: vec![PublishTrackConfig {
                    kind: TrackKind::Audio,
                    source: CaptureSource::Camera,
                    broadcast: None,
                    width: None,
                    height: None,
                    codec: None,
                    max_bitrate: None,
                }],
            }),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    fn case_duplicate_source() -> ClientSection {
        ClientSection {
            publish: Some(PublishConfig {
                broadcast: "bcast".into(),
                tracks: vec![
                    video_track(CaptureSource::Camera),
                    video_track(CaptureSource::Camera),
                ],
            }),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    fn case_empty_track_broadcast() -> ClientSection {
        ClientSection {
            publish: Some(PublishConfig {
                broadcast: "bcast".into(),
                tracks: vec![audio_track(Some(""))],
            }),
            gateway_path: Some("/gw".into()),
            ..ClientSection::default()
        }
    }

    const RULE_CASES: &[LintCase] = &[
        LintCase {
            rule: "mode-mismatch-dynamic",
            mode: EngineMode::Dynamic,
            build: case_mode_mismatch_dynamic,
        },
        LintCase {
            rule: "mode-mismatch-oneshot",
            mode: EngineMode::OneShot,
            build: case_mode_mismatch_oneshot,
        },
        LintCase {
            rule: "missing-gateway",
            mode: EngineMode::Dynamic,
            build: case_missing_gateway,
        },
        LintCase { rule: "watch-no-media", mode: EngineMode::Dynamic, build: case_watch_no_media },
        LintCase {
            rule: "input-none-with-accept",
            mode: EngineMode::OneShot,
            build: case_input_none_with_accept,
        },
        LintCase {
            rule: "input-trigger-with-accept",
            mode: EngineMode::OneShot,
            build: case_input_trigger_with_accept,
        },
        LintCase {
            rule: "field-hints-no-input",
            mode: EngineMode::OneShot,
            build: case_field_hints_no_input,
        },
        LintCase {
            rule: "asset-tags-no-input",
            mode: EngineMode::OneShot,
            build: case_asset_tags_no_input,
        },
        LintCase {
            rule: "text-no-placeholder",
            mode: EngineMode::OneShot,
            build: case_text_no_placeholder,
        },
        LintCase {
            rule: "empty-broadcast",
            mode: EngineMode::Dynamic,
            build: case_empty_broadcast,
        },
        LintCase {
            rule: "duplicate-broadcast",
            mode: EngineMode::Dynamic,
            build: case_duplicate_broadcast,
        },
        LintCase { rule: "empty-tracks", mode: EngineMode::Dynamic, build: case_empty_tracks },
        LintCase {
            rule: "kind-source-mismatch",
            mode: EngineMode::Dynamic,
            build: case_kind_source_mismatch,
        },
        LintCase {
            rule: "duplicate-source",
            mode: EngineMode::Dynamic,
            build: case_duplicate_source,
        },
        LintCase {
            rule: "empty-track-broadcast",
            mode: EngineMode::Dynamic,
            build: case_empty_track_broadcast,
        },
    ];

    #[test]
    fn each_documented_rule_fires_for_its_documented_input() {
        for case in RULE_CASES {
            let client = (case.build)();
            let warnings = lint_client_section(&client, case.mode);
            assert!(
                warnings.iter().any(|w| w.rule == case.rule),
                "expected rule `{}` for mode {:?}, got: {:?}",
                case.rule,
                case.mode,
                warnings.iter().map(|w| w.rule).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn clean_dynamic_input_fires_no_documented_rules() {
        let clean = ClientSection {
            gateway_path: Some("/gw".into()),
            publish: Some(publish_one_audio("uplink")),
            watch: Some(watch_audio_only()),
            ..ClientSection::default()
        };
        let warnings = lint_client_section(&clean, EngineMode::Dynamic);
        for case in RULE_CASES {
            assert!(
                !warnings.iter().any(|w| w.rule == case.rule),
                "rule `{}` unexpectedly fired on clean dynamic input: {:?}",
                case.rule,
                warnings.iter().map(|w| w.rule).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn clean_oneshot_input_fires_no_documented_rules() {
        let clean = ClientSection {
            input: Some(InputConfig {
                input_type: InputType::FileUpload,
                accept: Some("audio/*".into()),
                asset_tags: Some(vec!["speech".into()]),
                placeholder: None,
                field_hints: None,
            }),
            output: Some(OutputConfig { output_type: OutputType::Transcription }),
            ..ClientSection::default()
        };
        let warnings = lint_client_section(&clean, EngineMode::OneShot);
        for case in RULE_CASES {
            assert!(
                !warnings.iter().any(|w| w.rule == case.rule),
                "rule `{}` unexpectedly fired on clean oneshot input: {:?}",
                case.rule,
                warnings.iter().map(|w| w.rule).collect::<Vec<_>>()
            );
        }
    }

    // The `publish-no-media` rule documented at the top of this file is not
    // implemented: `PublishConfig` has no `audio`/`video` boolean fields,
    // only a `tracks` array.  Pin the current behaviour so the rustdoc/code
    // drift surfaces if anyone tries to revive the rule without re-reading
    // the model.  See PR description "Follow-ups / observations".
    #[test]
    fn publish_no_media_rule_is_not_emitted() {
        let clean = ClientSection {
            gateway_path: Some("/gw".into()),
            publish: Some(publish_one_audio("bcast")),
            ..ClientSection::default()
        };
        let warnings = lint_client_section(&clean, EngineMode::Dynamic);
        assert!(
            !warnings.iter().any(|w| w.rule == "publish-no-media"),
            "unexpected `publish-no-media` warning: {warnings:?}"
        );
    }

    #[test]
    fn watch_with_empty_broadcast_emits_empty_broadcast_rule() {
        let client = ClientSection {
            watch: Some(WatchConfig {
                broadcast: Some(String::new()),
                mse_path: None,
                audio: true,
                video: true,
            }),
            ..ClientSection::default()
        };
        assert!(ruled(&client, EngineMode::Dynamic, "empty-broadcast"));
    }

    #[test]
    fn asset_tags_with_text_input_emits_asset_tags_rule() {
        let client = ClientSection {
            input: Some(InputConfig {
                input_type: InputType::Text,
                accept: None,
                asset_tags: Some(vec!["speech".into()]),
                placeholder: Some("Say something".into()),
                field_hints: None,
            }),
            ..ClientSection::default()
        };
        assert!(ruled(&client, EngineMode::OneShot, "asset-tags-no-input"));
    }

    #[test]
    fn mode_mismatch_oneshot_includes_controls() {
        let client = ClientSection {
            controls: Some(vec![ControlConfig {
                label: "Vol".into(),
                control_type: ControlType::Toggle,
                node: "gain".into(),
                property: "gain".into(),
                group: None,
                default: None,
                min: None,
                max: None,
                step: None,
                value: None,
                options: None,
            }]),
            ..ClientSection::default()
        };
        assert!(ruled(&client, EngineMode::OneShot, "mode-mismatch-oneshot"));
    }
}
