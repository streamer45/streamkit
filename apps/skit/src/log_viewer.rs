// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Sse,
    },
    Json,
};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncSeekExt, BufReader};
use tracing::{debug, warn};

use crate::state::AppState;

/// Maximum number of lines that can be requested in a single page.
const MAX_LINE_LIMIT: usize = 5000;

/// Default number of lines per page.
const DEFAULT_LINE_LIMIT: usize = 500;

/// Interval between file polls during live tail (milliseconds).
const TAIL_POLL_INTERVAL_MS: u64 = 500;

/// Maximum number of lines to buffer per SSE event during live tail.
const TAIL_MAX_LINES_PER_EVENT: usize = 200;

/// Query parameters for paginated log retrieval with filtering.
#[derive(Deserialize)]
pub struct LogQuery {
    /// Byte offset to start reading from. Default: 0 for forward, end-of-file for backward.
    offset: Option<u64>,
    /// Maximum number of lines to return (default: 500, max: 5000).
    limit: Option<usize>,
    /// Reading direction: "forward" (default) or "backward".
    direction: Option<String>,
    /// Case-insensitive substring filter applied to each line.
    filter: Option<String>,
    /// Filter by log level: "error", "warn", "info", "debug", "trace".
    level: Option<String>,
}

/// Response for paginated log reading.
#[derive(Serialize)]
pub struct LogResponse {
    /// The log lines (after filtering).
    lines: Vec<String>,
    /// Byte offset for the next page in the current direction.
    next_offset: u64,
    /// Whether more data exists in the given direction.
    has_more: bool,
    /// Total log file size in bytes.
    file_size: u64,
}

/// Query parameters for the live-tail SSE stream.
#[derive(Deserialize)]
pub struct LogStreamQuery {
    /// Case-insensitive substring filter applied to each line.
    filter: Option<String>,
    /// Filter by log level: "error", "warn", "info", "debug", "trace".
    level: Option<String>,
}

/// Resolve the log file path from config, canonicalizing relative paths against cwd.
fn resolve_log_path(config_path: &str) -> Result<PathBuf, StatusCode> {
    let path = Path::new(config_path);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        let cwd = std::env::current_dir().map_err(|e| {
            warn!("Failed to get cwd for log path resolution: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        Ok(cwd.join(path))
    }
}

/// Check if a log line matches the given level filter.
///
/// Looks for the tracing level token (e.g. " INFO ", " WARN ") in the line.
/// For JSON-formatted logs, looks for `"level":"<LEVEL>"`.
fn matches_level(line: &str, level: &str) -> bool {
    let level_upper = level.to_ascii_uppercase();

    // Text format: "2024-01-01T00:00:00Z  INFO skit::server: ..."
    // The level token is typically surrounded by whitespace.
    let text_token = format!(" {level_upper} ");
    if line.contains(&text_token) {
        return true;
    }

    // Also match "  INFO " (double-space before, common in tracing output)
    let text_token_double = format!("  {level_upper} ");
    if line.contains(&text_token_double) {
        return true;
    }

    // JSON format: {"level":"INFO", ...}
    let json_token = format!("\"level\":\"{level_upper}\"");
    if line.contains(&json_token) {
        return true;
    }

    // JSON with lowercase
    let json_token_lower = format!("\"level\":\"{}\"", level.to_ascii_lowercase());
    line.contains(&json_token_lower)
}

/// Check if a line passes both filters (level + substring).
fn line_passes_filters(line: &str, level: Option<&str>, filter: Option<&str>) -> bool {
    if let Some(lvl) = level {
        if !lvl.is_empty() && !matches_level(line, lvl) {
            return false;
        }
    }
    if let Some(f) = filter {
        if !f.is_empty() && !line.to_ascii_lowercase().contains(&f.to_ascii_lowercase()) {
            return false;
        }
    }
    true
}

/// Handler: reads a page of log lines from the configured log file.
///
/// RBAC: requires `access_all_sessions` (admin only).
///
/// # Errors
///
/// Returns `StatusCode::FORBIDDEN` if the caller lacks admin permissions,
/// `StatusCode::NOT_FOUND` if file logging is disabled or the log file does not exist,
/// or `StatusCode::INTERNAL_SERVER_ERROR` on I/O failures.
pub async fn get_logs_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LogQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    // Check permissions — admin only
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.access_all_sessions {
        return Err(StatusCode::FORBIDDEN);
    }

    // Check that file logging is enabled
    if !app_state.config.log.file_enable {
        return Err(StatusCode::NOT_FOUND);
    }

    let log_path = resolve_log_path(&app_state.config.log.file_path)?;

    if !log_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let limit = query.limit.unwrap_or(DEFAULT_LINE_LIMIT).min(MAX_LINE_LIMIT);

    let direction = query.direction.as_deref().unwrap_or("forward");

    let file = tokio::fs::File::open(&log_path).await.map_err(|e| {
        warn!("Failed to open log file {}: {e}", log_path.display());
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let metadata = file.metadata().await.map_err(|e| {
        warn!("Failed to read log file metadata: {e}",);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let file_size = metadata.len();

    let response = if direction == "backward" {
        read_backward(
            file,
            file_size,
            query.offset,
            limit,
            query.level.as_deref(),
            query.filter.as_deref(),
        )
        .await?
    } else {
        read_forward(
            file,
            file_size,
            query.offset.unwrap_or(0),
            limit,
            query.level.as_deref(),
            query.filter.as_deref(),
        )
        .await?
    };

    debug!(
        lines = response.lines.len(),
        next_offset = response.next_offset,
        has_more = response.has_more,
        file_size = response.file_size,
        "Log viewer: served page"
    );

    Ok(Json(response))
}

/// Read lines forward from the given byte offset.
async fn read_forward(
    mut file: tokio::fs::File,
    file_size: u64,
    offset: u64,
    limit: usize,
    level: Option<&str>,
    filter: Option<&str>,
) -> Result<LogResponse, StatusCode> {
    let seek_to = offset.min(file_size);
    file.seek(std::io::SeekFrom::Start(seek_to)).await.map_err(|e| {
        warn!("Failed to seek log file: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let reader = BufReader::new(file);
    let mut lines_iter = reader.lines();
    let mut lines = Vec::with_capacity(limit.min(256));
    let mut bytes_read: u64 = 0;

    // If we started mid-file and the offset isn't 0, skip the first partial line
    if seek_to > 0 {
        if let Some(partial) = lines_iter.next_line().await.map_err(|e| {
            warn!("Failed to read log line: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })? {
            bytes_read += partial.len() as u64 + 1; // +1 for newline
        }
    }

    while lines.len() < limit {
        match lines_iter.next_line().await {
            Ok(Some(line)) => {
                bytes_read += line.len() as u64 + 1;
                if line_passes_filters(&line, level, filter) {
                    lines.push(line);
                }
            },
            Ok(None) => break,
            Err(e) => {
                warn!("Error reading log line: {e}");
                break;
            },
        }
    }

    let next_offset = seek_to + bytes_read;
    let has_more = next_offset < file_size;

    Ok(LogResponse { lines, next_offset, has_more, file_size })
}

/// Read lines backward from the given byte offset (or end of file).
///
/// Reads in chunks working backward from the offset, joining partial lines
/// at chunk boundaries via a carry buffer so no lines are lost.
async fn read_backward(
    mut file: tokio::fs::File,
    file_size: u64,
    offset: Option<u64>,
    limit: usize,
    level: Option<&str>,
    filter: Option<&str>,
) -> Result<LogResponse, StatusCode> {
    let end = offset.unwrap_or(file_size).min(file_size);

    if end == 0 {
        return Ok(LogResponse { lines: Vec::new(), next_offset: 0, has_more: false, file_size });
    }

    let mut collected: Vec<String> = Vec::with_capacity(limit.min(256));
    let mut current_end = end;
    // Partial line fragment carried from the start of the previously-read (higher-offset) chunk.
    // It is the continuation of the last split element of the current chunk.
    let mut carry = String::new();

    // Read in chunks working backward until we have enough lines or reach start of file
    while collected.len() < limit && current_end > 0 {
        // Read a chunk large enough to likely contain the lines we need.
        // Start with 8KB per remaining line needed, min 32KB.
        let remaining_lines = limit - collected.len();
        let chunk_size: u64 = (remaining_lines as u64 * 8192).max(32768).min(current_end);
        let chunk_start = current_end.saturating_sub(chunk_size);

        file.seek(std::io::SeekFrom::Start(chunk_start)).await.map_err(|e| {
            warn!("Failed to seek log file: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let read_len = usize::try_from(current_end - chunk_start).unwrap_or(usize::MAX);
        let mut buf = vec![0u8; read_len];
        file.read_exact(&mut buf).await.map_err(|e| {
            warn!("Failed to read log file chunk: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let text = String::from_utf8_lossy(&buf);
        let mut parts: Vec<&str> = text.split('\n').collect();

        // The last split element may be a partial line whose continuation is in `carry`.
        // Join them to form the complete line at this chunk's upper boundary.
        let last = parts.pop().unwrap_or("");
        let completed_tail =
            if carry.is_empty() { last.to_string() } else { format!("{last}{carry}") };

        // If we're not at the start of the file, the first split element is a partial line
        // whose beginning is in an earlier chunk. Save it as carry for the next iteration.
        if chunk_start > 0 && !parts.is_empty() {
            carry = parts.remove(0).to_string();
        } else {
            carry = String::new();
        }

        // `parts` now contains only complete lines (no boundary partials).
        // Filter them, then append the completed tail line.
        let mut chunk_lines: Vec<String> = parts
            .into_iter()
            .filter(|line| !line.is_empty() && line_passes_filters(line, level, filter))
            .map(String::from)
            .collect();

        if !completed_tail.is_empty() && line_passes_filters(&completed_tail, level, filter) {
            chunk_lines.push(completed_tail);
        }

        // Take from the end of chunk_lines to fill remaining capacity, then prepend to collected
        let take_count = remaining_lines.min(chunk_lines.len());
        let start_idx = chunk_lines.len().saturating_sub(take_count);
        let mut new_lines: Vec<String> = chunk_lines[start_idx..].to_vec();
        new_lines.append(&mut collected);
        collected = new_lines;

        current_end = chunk_start;
    }

    // Truncate to limit if we over-collected
    if collected.len() > limit {
        let excess = collected.len() - limit;
        collected = collected[excess..].to_vec();
    }

    Ok(LogResponse {
        lines: collected,
        next_offset: current_end,
        has_more: current_end > 0,
        file_size,
    })
}

/// SSE endpoint: streams new log lines as they are appended to the log file.
///
/// RBAC: requires `access_all_sessions` (admin only).
///
/// # Errors
///
/// Returns `StatusCode::FORBIDDEN` if the caller lacks admin permissions,
/// `StatusCode::NOT_FOUND` if file logging is disabled or the log file does not exist,
/// or `StatusCode::INTERNAL_SERVER_ERROR` on I/O failures.
pub async fn stream_logs_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<LogStreamQuery>,
) -> Result<Sse<impl futures::Stream<Item = Result<Event, Infallible>>>, StatusCode> {
    // Check permissions — admin only
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.access_all_sessions {
        return Err(StatusCode::FORBIDDEN);
    }

    if !app_state.config.log.file_enable {
        return Err(StatusCode::NOT_FOUND);
    }

    let log_path = resolve_log_path(&app_state.config.log.file_path)?;

    if !log_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let filter = query.filter;
    let level = query.level;

    let stream = async_stream::stream! {
        // Open file and seek to end
        let Ok(mut file) = tokio::fs::File::open(&log_path).await else {
            yield Ok(Event::default().data("[error] Failed to open log file"));
            return;
        };

        let Ok(metadata) = file.metadata().await else {
            yield Ok(Event::default().data("[error] Failed to read log file metadata"));
            return;
        };

        let mut last_size = metadata.len();
        if file.seek(std::io::SeekFrom::End(0)).await.is_err() {
            yield Ok(Event::default().data("[error] Failed to seek to end of log file"));
            return;
        }

        loop {
            tokio::time::sleep(tokio::time::Duration::from_millis(TAIL_POLL_INTERVAL_MS)).await;

            // Check current file size
            let Ok(metadata) = tokio::fs::metadata(&log_path).await else {
                continue;
            };
            let current_size = metadata.len();

            if current_size < last_size {
                // File was truncated (rotated) — reopen from start
                let Ok(new_file) = tokio::fs::File::open(&log_path).await else {
                    continue;
                };
                file = new_file;
                last_size = 0;
                yield Ok(Event::default().event("truncated").data("Log file was rotated"));
                continue;
            }

            if current_size == last_size {
                continue;
            }

            // Read new data
            let new_bytes = usize::try_from(current_size - last_size).unwrap_or(usize::MAX);
            let mut buf = vec![0u8; new_bytes];
            if file.read_exact(&mut buf).await.is_err() {
                // Re-seek if read fails
                let _ = file.seek(std::io::SeekFrom::Start(current_size)).await;
                last_size = current_size;
                continue;
            }

            last_size = current_size;

            let text = String::from_utf8_lossy(&buf);
            let new_lines: Vec<&str> = text.split('\n')
                .filter(|line| {
                    !line.is_empty()
                        && line_passes_filters(line, level.as_deref(), filter.as_deref())
                })
                .collect();

            if new_lines.is_empty() {
                continue;
            }

            // Send lines in batches to avoid overwhelming the client
            for chunk in new_lines.chunks(TAIL_MAX_LINES_PER_EVENT) {
                let payload = chunk.join("\n");
                yield Ok(Event::default().data(payload));
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_matches_level_text_format() {
        let line = "2024-01-01T00:00:00Z  INFO skit::server: Starting server";
        assert!(matches_level(line, "info"));
        assert!(matches_level(line, "INFO"));
        assert!(!matches_level(line, "warn"));
        assert!(!matches_level(line, "error"));
    }

    #[test]
    fn test_matches_level_json_format() {
        let line = r#"{"timestamp":"2024-01-01T00:00:00Z","level":"WARN","message":"test"}"#;
        assert!(matches_level(line, "warn"));
        assert!(matches_level(line, "WARN"));
        assert!(!matches_level(line, "info"));
    }

    #[test]
    fn test_line_passes_filters_no_filters() {
        assert!(line_passes_filters("any line", None, None));
    }

    #[test]
    fn test_line_passes_filters_level_only() {
        let line = "2024-01-01T00:00:00Z  WARN skit::server: Something happened";
        assert!(line_passes_filters(line, Some("warn"), None));
        assert!(!line_passes_filters(line, Some("error"), None));
    }

    #[test]
    fn test_line_passes_filters_text_only() {
        let line = "2024-01-01T00:00:00Z  INFO skit::server: Starting server";
        assert!(line_passes_filters(line, None, Some("starting")));
        assert!(line_passes_filters(line, None, Some("SERVER")));
        assert!(!line_passes_filters(line, None, Some("shutdown")));
    }

    #[test]
    fn test_line_passes_filters_both() {
        let line = "2024-01-01T00:00:00Z  INFO skit::server: Starting server";
        assert!(line_passes_filters(line, Some("info"), Some("starting")));
        assert!(!line_passes_filters(line, Some("warn"), Some("starting")));
        assert!(!line_passes_filters(line, Some("info"), Some("shutdown")));
    }

    #[test]
    fn test_line_passes_filters_empty_strings() {
        let line = "any line";
        assert!(line_passes_filters(line, Some(""), None));
        assert!(line_passes_filters(line, None, Some("")));
        assert!(line_passes_filters(line, Some(""), Some("")));
    }

    /// Verify that backward reading with a small chunk size (forcing multi-chunk)
    /// correctly joins partial lines at chunk boundaries instead of dropping them.
    #[tokio::test]
    async fn test_read_backward_multi_chunk_preserves_lines() {
        use tokio::io::AsyncWriteExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");

        // Write enough lines so that a small forced chunk triggers multi-chunk reading.
        let mut f = tokio::fs::File::create(&path).await.unwrap();
        let mut expected: Vec<String> = Vec::new();
        for i in 0..20 {
            let line = format!("2024-01-01T00:00:00Z  INFO test: line number {i}");
            f.write_all(line.as_bytes()).await.unwrap();
            f.write_all(b"\n").await.unwrap();
            expected.push(line);
        }
        f.flush().await.unwrap();
        drop(f);

        let file = tokio::fs::File::open(&path).await.unwrap();
        let file_size = file.metadata().await.unwrap().len();

        let resp = read_backward(file, file_size, None, 100, None, None).await.unwrap();

        assert_eq!(resp.lines, expected, "all lines should be returned in order");
        assert!(!resp.has_more);
    }
}
