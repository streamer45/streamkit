// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use multer as raw_multer;
use opentelemetry::{global, KeyValue};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::state::AppState;
use streamkit_api::yaml::{compile, UserPipeline};
use streamkit_api::Pipeline;
use streamkit_engine::{OneshotEngineConfig, OneshotInput};

use super::AppError;

/// Type alias for a boxed byte stream used in media processing
type MediaStream = Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Unpin + Send>;

static ONESHOT_DURATION_HISTOGRAM: OnceLock<opentelemetry::metrics::Histogram<f64>> =
    OnceLock::new();

/// Binding between a multipart field and an http_input node.
struct HttpInputBinding {
    node_id: String,
    field_name: String,
    output_pin: String,
    required: bool,
}

/// Combine the per-request `status` with the resolved bounded labels.
fn duration_labels(status: &'static str, extra: &[KeyValue]) -> Vec<KeyValue> {
    let mut labels = Vec::with_capacity(extra.len() + 1);
    labels.push(KeyValue::new(crate::metrics_labels::STATUS_KEY, status));
    labels.extend_from_slice(extra);
    labels
}

/// Extract content-type header and multipart boundary from request headers.
fn extract_multipart_boundary(headers: &HeaderMap) -> Result<String, AppError> {
    let ct_header = headers
        .get(header::CONTENT_TYPE)
        .ok_or_else(|| AppError::BadRequest("Missing Content-Type header".to_string()))
        .and_then(|hv| {
            hv.to_str().map_err(|_| AppError::BadRequest("Invalid Content-Type header".to_string()))
        })?;
    raw_multer::parse_boundary(ct_header)
        .map_err(|e| AppError::BadRequest(format!("Invalid multipart boundary: {e}")))
}

/// Parse and validate the first multipart field as config.
async fn parse_config_field(
    multipart: &mut raw_multer::Multipart<'_>,
) -> Result<UserPipeline, AppError> {
    tracing::debug!("Parsing first multipart field");
    let first_field = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
        .ok_or_else(|| AppError::BadRequest("Empty multipart payload".to_string()))?;
    let first_name = first_field.name().map(std::string::ToString::to_string).unwrap_or_default();
    if first_name != "config" {
        return Err(AppError::BadRequest(
            "Multipart fields must be ordered: 'config' first".to_string(),
        ));
    }

    let config_bytes = first_field
        .bytes()
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read config field: {e}")))?;
    let yaml_str = std::str::from_utf8(&config_bytes)
        .map_err(|e| AppError::BadRequest(format!("Config is not valid UTF-8: {e}")))?;
    streamkit_api::yaml::parse_yaml(yaml_str).map_err(AppError::BadRequest)
}

/// Build http_input bindings from the pipeline definition.
///
/// Defaults:
/// - Single http_input: field name defaults to "media"
/// - Multiple http_input: field names default to the node id
fn determine_http_input_bindings(
    pipeline_def: &Pipeline,
) -> Result<Vec<HttpInputBinding>, AppError> {
    // Record which output pins the pipeline references for each http_input node
    let mut pins_used: HashMap<String, HashSet<String>> = HashMap::new();
    for conn in &pipeline_def.connections {
        if let Some(node_def) = pipeline_def.nodes.get(&conn.from_node) {
            if node_def.kind == "streamkit::http_input" {
                pins_used.entry(conn.from_node.clone()).or_default().insert(conn.from_pin.clone());
            }
        }
    }

    let http_inputs: Vec<(&String, &streamkit_api::Node)> = pipeline_def
        .nodes
        .iter()
        .filter(|(_, node)| node.kind == "streamkit::http_input")
        .collect();

    let default_field = if http_inputs.len() == 1 { Some("media".to_string()) } else { None };
    let mut seen_fields: HashSet<String> = HashSet::new();
    let mut bindings = Vec::new();

    for (node_id, node_def) in http_inputs {
        let mut node_bindings: Vec<HttpInputBinding> = Vec::new();
        let mut single_field: Option<String> = None;
        let mut single_required = true;
        let mut has_fields_param = false;
        let mut has_single_field_param = false;

        if let Some(params) = &node_def.params {
            if let Some(fields_val) = params.get("fields") {
                has_fields_param = true;
                let fields = fields_val.as_array().ok_or_else(|| {
                    AppError::BadRequest(
                        "streamkit::http_input.params.fields must be an array of strings or objects"
                            .to_string(),
                    )
                })?;

                for entry in fields {
                    let (name, required) = match entry {
                        serde_json::Value::String(s) => (s.clone(), true),
                        serde_json::Value::Object(map) => {
                            let Some(name_val) = map.get("name") else {
                                return Err(AppError::BadRequest(
                                    "fields entries must include 'name'".to_string(),
                                ));
                            };
                            let name = name_val
                                .as_str()
                                .ok_or_else(|| {
                                    AppError::BadRequest("fields.name must be a string".to_string())
                                })?
                                .trim()
                                .to_string();
                            if name.is_empty() {
                                return Err(AppError::BadRequest(
                                    "fields.name must not be empty".to_string(),
                                ));
                            }
                            let required = map
                                .get("required")
                                .and_then(serde_json::Value::as_bool)
                                .unwrap_or(true);
                            (name, required)
                        },
                        _ => {
                            return Err(AppError::BadRequest(
                                "fields entries must be strings or objects".to_string(),
                            ))
                        },
                    };

                    node_bindings.push(HttpInputBinding {
                        node_id: node_id.clone(),
                        field_name: name,
                        output_pin: String::new(),
                        required,
                    });
                }
            } else if let Some(field_val) = params.get("field").and_then(serde_json::Value::as_str)
            {
                has_single_field_param = true;
                let trimmed = field_val.trim();
                if !trimmed.is_empty() {
                    single_field = Some(trimmed.to_string());
                }
                if let Some(req_val) = params.get("required").and_then(serde_json::Value::as_bool) {
                    single_required = req_val;
                }
            }
        }

        if has_fields_param && has_single_field_param {
            return Err(AppError::BadRequest(
                "streamkit::http_input: use either 'field' or 'fields', not both".to_string(),
            ));
        }

        if has_fields_param && node_bindings.is_empty() {
            return Err(AppError::BadRequest(
                "streamkit::http_input.params.fields must include at least one field".to_string(),
            ));
        }

        if node_bindings.is_empty() {
            let field_name =
                single_field.or_else(|| default_field.clone()).unwrap_or_else(|| node_id.clone());
            node_bindings.push(HttpInputBinding {
                node_id: node_id.clone(),
                field_name,
                output_pin: String::new(),
                required: single_required,
            });
        }

        // Back-compat: allow implicit 'media' only when no fields array is provided.
        if !has_fields_param
            && default_field.as_deref() == Some("media")
            && !node_bindings.iter().any(|b| b.field_name == "media")
        {
            node_bindings.push(HttpInputBinding {
                node_id: node_id.clone(),
                field_name: "media".to_string(),
                output_pin: String::new(),
                required: false,
            });
        }

        // Decide pin names based on referenced connections. Keep field names for multi-field mode,
        // but allow legacy 'out' default when only one pin is referenced (steps format).
        let used_pins = pins_used.get(node_id.as_str()).cloned().unwrap_or_default();
        for binding in &mut node_bindings {
            let pin_name = if used_pins.contains(&binding.field_name) {
                binding.field_name.clone()
            } else if used_pins.len() == 1 && !has_fields_param {
                // Legacy steps pipelines reference 'out'
                used_pins.iter().next().cloned().unwrap_or_else(|| binding.field_name.clone())
            } else {
                binding.field_name.clone()
            };
            binding.output_pin = pin_name;
        }

        for binding in node_bindings {
            if !seen_fields.insert(binding.field_name.clone()) {
                return Err(AppError::BadRequest(format!(
                    "Duplicate multipart field name '{field_name}' across http_input nodes",
                    field_name = binding.field_name
                )));
            }
            bindings.push(binding);
        }
    }

    Ok(bindings)
}

/// Stream all chunks from a media field through the provided channel.
async fn stream_media_field_chunks(
    field: &mut raw_multer::Field<'_>,
    media_tx: &tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>,
    cancellation_token: Option<&CancellationToken>,
) {
    let mut chunk_count: usize = 0;
    let mut total_bytes: usize = 0;

    if let Some(token) = cancellation_token {
        loop {
            tokio::select! {
                () = token.cancelled() => {
                    tracing::info!(
                        "Stopped streaming media early after {} chunks ({} bytes) due to cancellation",
                        chunk_count,
                        total_bytes
                    );
                    break;
                }
                chunk_result = field.chunk() => {
                    match chunk_result {
                        Ok(Some(chunk)) => {
                            chunk_count += 1;
                            total_bytes += chunk.len();
                            if media_tx.send(Ok(chunk)).await.is_err() {
                                tracing::debug!(
                                    "Media consumer dropped after {} chunks ({} bytes)",
                                    chunk_count,
                                    total_bytes
                                );
                                break;
                            }
                        },
                        Ok(None) => {
                            tracing::info!(
                                "Finished streaming media after {} chunks ({} bytes)",
                                chunk_count,
                                total_bytes
                            );
                            break;
                        },
                        Err(e) => {
                            let _ = media_tx.send(Err(axum::Error::new(e))).await;
                            break;
                        },
                    }
                }
            }
        }
        return;
    }

    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                chunk_count += 1;
                total_bytes += chunk.len();
                if media_tx.send(Ok(chunk)).await.is_err() {
                    tracing::debug!(
                        "Media consumer dropped after {} chunks ({} bytes)",
                        chunk_count,
                        total_bytes
                    );
                    break;
                }
            },
            Ok(None) => {
                tracing::info!(
                    "Finished streaming media after {} chunks ({} bytes)",
                    chunk_count,
                    total_bytes
                );
                break;
            },
            Err(e) => {
                let _ = media_tx.send(Err(axum::Error::new(e))).await;
                break;
            },
        }
    }
}

/// Route multipart fields into pre-created channels based on expected names.
async fn route_multipart_fields(
    mut multipart: raw_multer::Multipart<'_>,
    mut field_senders: HashMap<String, tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>>,
    required_fields: HashSet<String>,
    mut required_seen_tx: Option<tokio::sync::oneshot::Sender<()>>,
    parse_done_tx: tokio::sync::oneshot::Sender<Result<(), AppError>>,
    cancellation_token: CancellationToken,
) {
    let mut seen_required: HashSet<String> = HashSet::new();

    let result = async {
        while let Some(mut field) = multipart
            .next_field()
            .await
            .map_err(|e| AppError::BadRequest(format!("Multipart error: {e}")))?
        {
            let fname = field.name().map(std::string::ToString::to_string).unwrap_or_default();
            if fname.is_empty() {
                continue;
            }

            let Some(sender) = field_senders.remove(&fname) else {
                let expected = if field_senders.is_empty() {
                    "none".to_string()
                } else {
                    field_senders.keys().cloned().collect::<Vec<_>>().join(", ")
                };
                return Err(AppError::BadRequest(format!(
                    "Unexpected multipart field '{fname}'. Expected: {expected}"
                )));
            };

            if required_fields.contains(&fname) {
                seen_required.insert(fname.clone());
                if seen_required.len() == required_fields.len() {
                    if let Some(tx) = required_seen_tx.take() {
                        let _ = tx.send(());
                    }
                }
            }

            stream_media_field_chunks(&mut field, &sender, Some(&cancellation_token)).await;
        }

        if !required_fields.is_empty() && seen_required.len() < required_fields.len() {
            let missing: Vec<_> = required_fields.difference(&seen_required).cloned().collect();
            return Err(AppError::BadRequest(format!(
                "Missing required multipart field(s): {}",
                missing.join(", ")
            )));
        }

        Ok(())
    }
    .await;

    drop(field_senders);

    if let Some(tx) = required_seen_tx.take() {
        let _ = tx.send(());
    }

    let _ = parse_done_tx.send(result);
}

/// Build HTTP response from pipeline execution result.
fn build_streaming_response(
    pipeline_result: streamkit_engine::OneshotPipelineResult,
    start_time: Instant,
    duration_histogram: opentelemetry::metrics::Histogram<f64>,
    attributes: Arc<streamkit_engine::ResolvedAttributes>,
) -> Response {
    tracing::debug!(
        "Creating streaming response with content type: {}",
        pipeline_result.content_type
    );

    let stream = ReceiverStream::new(pipeline_result.data_stream).map(Ok::<_, Infallible>);
    let stream = InstrumentedOneshotStream::new(stream, start_time, duration_histogram, attributes);
    let body = Body::from_stream(stream);

    let mut headers = HeaderMap::new();
    match pipeline_result.content_type.parse() {
        Ok(ct) => headers.insert("Content-Type", ct),
        Err(e) => {
            tracing::error!(
                content_type = %pipeline_result.content_type,
                error = %e,
                "Failed to parse content type from pipeline output, using fallback"
            );
            // Fallback MIME type is a constant string that will always parse successfully
            #[allow(clippy::expect_used)]
            headers.insert(
                "Content-Type",
                "application/octet-stream".parse().expect("fallback MIME type should always parse"),
            )
        },
    };

    tracing::info!("Returning streaming response to client");
    (headers, body).into_response()
}

struct InstrumentedOneshotStream<S> {
    inner: S,
    start_time: Instant,
    recorded: bool,
    duration_histogram: opentelemetry::metrics::Histogram<f64>,
    attributes: Arc<streamkit_engine::ResolvedAttributes>,
}

impl<S> InstrumentedOneshotStream<S> {
    const fn new(
        inner: S,
        start_time: Instant,
        duration_histogram: opentelemetry::metrics::Histogram<f64>,
        attributes: Arc<streamkit_engine::ResolvedAttributes>,
    ) -> Self {
        Self { inner, start_time, recorded: false, duration_histogram, attributes }
    }

    fn record(&mut self, status: &'static str) {
        if self.recorded {
            return;
        }
        self.recorded = true;
        let labels = duration_labels(status, &self.attributes.pipeline);
        self.duration_histogram.record(self.start_time.elapsed().as_secs_f64(), &labels);
    }
}

impl<S> Drop for InstrumentedOneshotStream<S> {
    fn drop(&mut self) {
        if !self.recorded {
            self.record("incomplete");
        }
    }
}

impl<S> Stream for InstrumentedOneshotStream<S>
where
    S: Stream<Item = Result<Bytes, Infallible>> + Unpin,
{
    type Item = Result<Bytes, Infallible>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(None) => {
                self.record("ok");
                Poll::Ready(None)
            },
            other => other,
        }
    }
}

/// The Axum handler for a oneshot multipart processing request.
// splitting would require threading cancel_token + engine state through many closures
#[allow(clippy::cognitive_complexity)]
pub(super) async fn process_oneshot_pipeline_handler(
    State(app_state): State<Arc<AppState>>,
    req: axum::extract::Request<Body>,
) -> Result<Response, AppError> {
    tracing::info!("Processing multipart request");

    let headers = req.headers().clone();
    let (role_name, perms) = crate::role_extractor::get_role_and_permissions(&headers, &app_state);
    if !perms.create_sessions {
        return Err(AppError::Forbidden(
            "Permission denied: cannot execute oneshot pipelines".to_string(),
        ));
    }

    let boundary = extract_multipart_boundary(req.headers())?;
    let body_stream = req.into_body().into_data_stream();
    let mut multipart = raw_multer::Multipart::new(body_stream, boundary);
    let user_pipeline = parse_config_field(&mut multipart).await?;

    let pipeline_def: Pipeline = compile(user_pipeline)?;

    let resolved_attributes = Arc::new(app_state.resolve_metric_attributes(&pipeline_def));

    let input_bindings = determine_http_input_bindings(&pipeline_def)?;

    let (has_http_input, has_file_read, has_http_output) =
        super::validation::validate_pipeline_nodes(&pipeline_def)?;

    // Enforce allowed node/plugin kinds for oneshot execution.
    //
    // Note: `streamkit::http_input` and `streamkit::http_output` are oneshot-only marker nodes,
    // but they are not part of the general `allowed_nodes` allowlist. Treat them as implicitly
    // allowed when oneshot execution itself is permitted.
    for (node_id, node_def) in &pipeline_def.nodes {
        let kind = node_def.kind.as_str();
        if kind == "streamkit::http_input" || kind == "streamkit::http_output" {
            continue;
        }

        if !perms.is_node_allowed(kind) {
            return Err(AppError::Forbidden(format!(
                "Permission denied: node type '{kind}' not allowed (node '{node_id}')"
            )));
        }

        if kind.starts_with("plugin::") && !perms.is_plugin_allowed(kind) {
            return Err(AppError::Forbidden(format!(
                "Permission denied: plugin '{kind}' not allowed (node '{node_id}')"
            )));
        }
    }

    let policy = app_state.file_security_policy();
    super::validation::validate_file_reader_paths(&pipeline_def, policy)?;
    super::validation::validate_file_writer_paths(&pipeline_def, policy)?;
    super::validation::validate_script_paths(&pipeline_def, policy)?;

    tracing::info!(
        "Pipeline validation passed: mode={}, has_http_input={}, has_file_read={}, has_http_output={}",
        if has_http_input { "http-streaming" } else if has_file_read { "file-based" } else { "generator" },
        has_http_input,
        has_file_read,
        has_http_output
    );
    tracing::info!(role = %role_name, "Executing oneshot pipeline for role");

    let cancel_token = CancellationToken::new();
    let mut field_senders: HashMap<String, tokio::sync::mpsc::Sender<Result<Bytes, axum::Error>>> =
        HashMap::new();
    let mut engine_inputs = Vec::new();
    let mut required_fields: HashSet<String> = HashSet::new();

    let io_capacity = app_state
        .config
        .engine
        .oneshot
        .io_channel_capacity
        .unwrap_or(streamkit_engine::constants::DEFAULT_ONESHOT_IO_CAPACITY);

    for binding in &input_bindings {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, axum::Error>>(io_capacity);
        if binding.required {
            required_fields.insert(binding.field_name.clone());
        }
        field_senders.insert(binding.field_name.clone(), tx);

        let media_stream: MediaStream = Box::new(ReceiverStream::new(rx).map(|x| x));
        engine_inputs.push(OneshotInput {
            node_id: binding.node_id.clone(),
            output_pin: binding.output_pin.clone(),
            stream: media_stream,
            content_type: None,
            field_name: binding.field_name.clone(),
            required: binding.required,
            cancellation_token: Some(cancel_token.clone()),
        });
    }

    let (required_seen_tx, required_seen_rx) = tokio::sync::oneshot::channel();
    let mut required_seen_tx = Some(required_seen_tx);
    if required_fields.is_empty() {
        if let Some(tx) = required_seen_tx.take() {
            let _ = tx.send(());
        }
    }
    let (parse_done_tx, parse_done_rx) = tokio::sync::oneshot::channel();

    let routing_task = tokio::spawn(route_multipart_fields(
        multipart,
        field_senders,
        required_fields.clone(),
        required_seen_tx,
        parse_done_tx,
        cancel_token.clone(),
    ));

    // Wait for required fields to appear (prevents hanging on missing uploads)
    tokio::time::timeout(Duration::from_secs(5), required_seen_rx)
        .await
        .map_err(|_| {
            cancel_token.cancel();
            AppError::BadRequest("Timed out waiting for required multipart fields".to_string())
        })?
        .map_err(|_| AppError::BadRequest("Failed to observe multipart state".into()))?;

    tracing::info!("Starting oneshot pipeline execution");
    let oneshot_start_time = Instant::now();
    let oneshot_duration_histogram = ONESHOT_DURATION_HISTOGRAM
        .get_or_init(|| {
            global::meter("skit_engine")
                .f64_histogram("oneshot_pipeline.duration")
                .with_description(
                    "Oneshot pipeline runtime from request start until response stream ends",
                )
                .with_boundaries(
                    streamkit_core::metrics::HISTOGRAM_BOUNDARIES_PIPELINE_DURATION.to_vec(),
                )
                .build()
        })
        .clone();

    let oneshot_config = {
        let cfg = &app_state.config.engine.oneshot;
        OneshotEngineConfig {
            packet_batch_size: cfg.packet_batch_size,
            media_channel_capacity: cfg
                .media_channel_capacity
                .unwrap_or(streamkit_engine::constants::DEFAULT_ONESHOT_MEDIA_CAPACITY),
            io_channel_capacity: cfg
                .io_channel_capacity
                .unwrap_or(streamkit_engine::constants::DEFAULT_ONESHOT_IO_CAPACITY),
            asset_root: app_state.asset_root.clone(),
        }
    };

    let pipeline_result = match app_state
        .engine
        .run_oneshot_pipeline(
            pipeline_def,
            engine_inputs,
            Some(oneshot_config),
            Arc::clone(&resolved_attributes),
            Some(cancel_token.clone()),
        )
        .await
    {
        Ok(result) => {
            tracing::info!("Oneshot pipeline execution completed");
            result
        },
        Err(e) => {
            let labels = duration_labels("error", &resolved_attributes.pipeline);
            oneshot_duration_histogram.record(oneshot_start_time.elapsed().as_secs_f64(), &labels);
            cancel_token.cancel();
            return Err(e.into());
        },
    };

    match parse_done_rx.await {
        Ok(Ok(())) => {},
        Ok(Err(err)) => {
            let labels = duration_labels("error", &resolved_attributes.pipeline);
            oneshot_duration_histogram.record(oneshot_start_time.elapsed().as_secs_f64(), &labels);
            cancel_token.cancel();
            return Err(err);
        },
        Err(e) => {
            let labels = duration_labels("error", &resolved_attributes.pipeline);
            oneshot_duration_histogram.record(oneshot_start_time.elapsed().as_secs_f64(), &labels);
            cancel_token.cancel();
            return Err(AppError::BadRequest(format!("Multipart routing task aborted: {e}")));
        },
    }
    let _ = routing_task.await;

    Ok(build_streaming_response(
        pipeline_result,
        oneshot_start_time,
        oneshot_duration_histogram,
        resolved_attributes,
    ))
}

#[cfg(test)]
// Tests use `expect` so setup failures fail loudly with a stable message; the
// production-code `expect_used` lint stays on everywhere else.
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use serde_json::json;
    use streamkit_api::{Connection, ConnectionMode, Node, Pipeline};

    fn header_map(content_type: Option<&str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = content_type {
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_str(value).expect("valid header value"),
            );
        }
        headers
    }

    fn bad_request(err: AppError) -> String {
        match err {
            AppError::BadRequest(msg) => msg,
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    fn expect_bindings_bad_request(result: Result<Vec<HttpInputBinding>, AppError>) -> String {
        match result {
            Ok(bindings) => panic!("expected BadRequest, got Ok with {} bindings", bindings.len()),
            Err(e) => bad_request(e),
        }
    }

    fn http_input(params: Option<serde_json::Value>) -> Node {
        Node { kind: "streamkit::http_input".to_string(), params, state: None }
    }

    fn sink_node() -> Node {
        Node { kind: "streamkit::http_output".to_string(), params: None, state: None }
    }

    fn connection(from_node: &str, from_pin: &str) -> Connection {
        Connection {
            from_node: from_node.to_string(),
            from_pin: from_pin.to_string(),
            to_node: "sink".to_string(),
            to_pin: "in".to_string(),
            mode: ConnectionMode::default(),
        }
    }

    fn pipeline_with(nodes: Vec<(&str, Node)>, connections: Vec<Connection>) -> Pipeline {
        let mut p = Pipeline::default();
        for (name, node) in nodes {
            p.nodes.insert(name.to_string(), node);
        }
        p.connections = connections;
        p
    }

    #[test]
    fn boundary_missing_content_type_is_bad_request() {
        let err = extract_multipart_boundary(&header_map(None)).expect_err("must fail");
        let msg = bad_request(err);
        assert!(msg.starts_with("Missing Content-Type"), "unexpected msg: {msg}");
    }

    #[test]
    fn boundary_invalid_header_bytes_is_bad_request() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_bytes(b"multipart/form-data; boundary=\xFF").expect("bytes header"),
        );
        let err = extract_multipart_boundary(&headers).expect_err("must fail");
        let msg = bad_request(err);
        assert!(msg.starts_with("Invalid Content-Type"), "unexpected msg: {msg}");
    }

    #[test]
    fn boundary_non_multipart_content_type_is_bad_request_mentioning_multipart() {
        let err = extract_multipart_boundary(&header_map(Some("application/json")))
            .expect_err("must fail");
        let msg = bad_request(err);
        assert!(msg.starts_with("Invalid multipart boundary"), "unexpected msg: {msg}");
        assert!(msg.to_lowercase().contains("multipart"), "msg should mention multipart: {msg}");
    }

    #[test]
    fn boundary_extracted_from_simple_content_type() {
        let value =
            extract_multipart_boundary(&header_map(Some("multipart/form-data; boundary=abcd1234")))
                .expect("ok");
        assert_eq!(value, "abcd1234");
    }

    #[test]
    fn boundary_strips_surrounding_quotes() {
        let value =
            extract_multipart_boundary(&header_map(Some("multipart/form-data; boundary=\"abcd\"")))
                .expect("ok");
        assert_eq!(value, "abcd");
    }

    #[test]
    fn boundary_parsed_regardless_of_parameter_order() {
        let value = extract_multipart_boundary(&header_map(Some(
            "multipart/form-data; charset=utf-8; boundary=xyz789",
        )))
        .expect("ok");
        assert_eq!(value, "xyz789");
    }

    fn single_input_pipeline(params: Option<serde_json::Value>) -> Pipeline {
        pipeline_with(vec![("input", http_input(params)), ("sink", sink_node())], vec![])
    }

    #[test]
    fn single_http_input_no_params_yields_implicit_media_binding() {
        let pipeline = single_input_pipeline(None);
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert_eq!(bindings.len(), 1);
        let b = &bindings[0];
        assert_eq!(b.node_id, "input");
        assert_eq!(b.field_name, "media");
        // NOTE: the production code currently reports required=true here even
        // though the back-compat path elsewhere treats implicit media as
        // optional. See follow-ups in the PR description.
        assert!(b.required);
    }

    #[test]
    fn single_http_input_with_explicit_field_adds_back_compat_media_binding() {
        let pipeline = single_input_pipeline(Some(json!({"field": "audio"})));
        let mut bindings = determine_http_input_bindings(&pipeline).expect("ok");
        bindings.sort_by(|a, b| a.field_name.cmp(&b.field_name));

        assert_eq!(bindings.len(), 2, "back-compat implicit 'media' is appended");
        let audio = &bindings[0];
        assert_eq!(audio.field_name, "audio");
        assert!(audio.required, "explicit single-field default is required=true");

        let media = &bindings[1];
        assert_eq!(media.field_name, "media");
        assert!(!media.required, "implicit media back-compat binding is optional");
    }

    #[test]
    fn single_http_input_with_field_and_required_false_respects_flag() {
        let pipeline = single_input_pipeline(Some(json!({"field": "audio", "required": false})));
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        let audio = bindings.iter().find(|b| b.field_name == "audio").expect("audio binding");
        assert!(!audio.required);
    }

    #[test]
    fn multiple_http_inputs_default_field_name_to_node_id() {
        let pipeline = pipeline_with(
            vec![("alpha", http_input(None)), ("beta", http_input(None)), ("sink", sink_node())],
            vec![],
        );
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert_eq!(bindings.len(), 2);
        let mut by_node: HashMap<&str, &HttpInputBinding> = HashMap::new();
        for b in &bindings {
            by_node.insert(b.node_id.as_str(), b);
        }
        assert_eq!(by_node["alpha"].field_name, "alpha");
        assert_eq!(by_node["beta"].field_name, "beta");
        assert!(by_node["alpha"].required && by_node["beta"].required);
    }

    #[test]
    fn fields_array_of_strings_makes_required_bindings() {
        let pipeline = single_input_pipeline(Some(json!({"fields": ["audio", "video"]})));
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert_eq!(bindings.len(), 2);
        for b in &bindings {
            assert!(b.required, "string entries default to required=true");
            assert_eq!(b.node_id, "input");
        }
        let names: HashSet<&str> = bindings.iter().map(|b| b.field_name.as_str()).collect();
        assert!(names.contains("audio") && names.contains("video"));
    }

    #[test]
    fn fields_array_of_objects_respects_required_flag() {
        let pipeline = single_input_pipeline(Some(json!({
            "fields": [
                {"name": "audio", "required": false},
                {"name": "video"},
            ]
        })));
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        let audio = bindings.iter().find(|b| b.field_name == "audio").expect("audio");
        let video = bindings.iter().find(|b| b.field_name == "video").expect("video");
        assert!(!audio.required);
        assert!(video.required, "object without 'required' defaults to true");
    }

    #[test]
    fn fields_object_name_is_trimmed() {
        let pipeline = single_input_pipeline(Some(json!({
            "fields": [{"name": "  audio  "}]
        })));
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].field_name, "audio");
    }

    #[test]
    fn fields_empty_array_is_bad_request() {
        let pipeline = single_input_pipeline(Some(json!({"fields": []})));
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.contains("at least one field"), "unexpected msg: {msg}");
    }

    #[test]
    fn fields_non_array_is_bad_request() {
        let pipeline = single_input_pipeline(Some(json!({"fields": "audio"})));
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.contains("must be an array"), "unexpected msg: {msg}");
    }

    // Pins current behavior when both `field` and `fields` are provided. The
    // production code's if-let / else-if-let branching silently ignores `field`
    // when `fields` is present; the documented intent (see PR follow-ups) is to
    // return BadRequest. This test exists so a future fix is intentional.
    #[test]
    fn both_field_and_fields_currently_uses_fields_and_ignores_field() {
        let pipeline = single_input_pipeline(Some(json!({
            "field": "ignored",
            "fields": ["audio"],
        })));
        let bindings = determine_http_input_bindings(&pipeline).expect("ok (currently)");
        let names: Vec<&str> = bindings.iter().map(|b| b.field_name.as_str()).collect();
        assert!(names.contains(&"audio"), "fields wins: {names:?}");
        assert!(!names.contains(&"ignored"), "field branch silently dropped: {names:?}");
    }

    #[test]
    fn fields_object_without_name_is_bad_request() {
        let pipeline = single_input_pipeline(Some(json!({"fields": [{"required": true}]})));
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.contains("must include 'name'"), "unexpected msg: {msg}");
    }

    #[test]
    fn fields_object_with_empty_name_is_bad_request() {
        let pipeline = single_input_pipeline(Some(json!({"fields": [{"name": "   "}]})));
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.contains("must not be empty"), "unexpected msg: {msg}");
    }

    #[test]
    fn fields_object_with_non_string_name_is_bad_request() {
        let pipeline = single_input_pipeline(Some(json!({"fields": [{"name": 42}]})));
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.contains("must be a string"), "unexpected msg: {msg}");
    }

    #[test]
    fn fields_entry_of_unsupported_type_is_bad_request() {
        let pipeline = single_input_pipeline(Some(json!({"fields": [42]})));
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.contains("must be strings or objects"), "unexpected msg: {msg}");
    }

    #[test]
    fn duplicate_field_name_across_nodes_is_bad_request() {
        let pipeline = pipeline_with(
            vec![
                ("alpha", http_input(Some(json!({"field": "shared"})))),
                ("beta", http_input(Some(json!({"field": "shared"})))),
                ("sink", sink_node()),
            ],
            vec![],
        );
        let msg = expect_bindings_bad_request(determine_http_input_bindings(&pipeline));
        assert!(msg.starts_with("Duplicate multipart field name"), "unexpected msg: {msg}");
    }

    #[test]
    fn pin_name_matches_field_when_connection_references_it() {
        let pipeline = pipeline_with(
            vec![
                ("input", http_input(Some(json!({"fields": ["audio", "video"]})))),
                ("sink", sink_node()),
            ],
            vec![connection("input", "audio")],
        );
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        let audio = bindings.iter().find(|b| b.field_name == "audio").expect("audio");
        let video = bindings.iter().find(|b| b.field_name == "video").expect("video");
        assert_eq!(audio.output_pin, "audio", "matching pin name wins");
        assert_eq!(
            video.output_pin, "video",
            "non-referenced field falls back to its own field_name in fields-mode"
        );
    }

    #[test]
    fn pin_name_falls_back_to_legacy_out_for_single_binding_steps_format() {
        let pipeline = pipeline_with(
            vec![("input", http_input(None)), ("sink", sink_node())],
            vec![connection("input", "out")],
        );
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].field_name, "media");
        assert_eq!(
            bindings[0].output_pin, "out",
            "single legacy 'out' pin should override default field_name pin"
        );
    }

    #[test]
    fn pin_name_does_not_apply_legacy_out_fallback_when_fields_param_is_set() {
        let pipeline = pipeline_with(
            vec![("input", http_input(Some(json!({"fields": ["audio"]})))), ("sink", sink_node())],
            vec![connection("input", "out")],
        );
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].field_name, "audio");
        assert_eq!(
            bindings[0].output_pin, "audio",
            "fields-mode never collapses to a legacy 'out' pin"
        );
    }

    #[test]
    fn no_http_inputs_returns_empty_bindings() {
        let pipeline = pipeline_with(vec![("sink", sink_node())], vec![]);
        let bindings = determine_http_input_bindings(&pipeline).expect("ok");
        assert!(bindings.is_empty());
    }
}
