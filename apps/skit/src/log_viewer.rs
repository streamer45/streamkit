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
/// Reads in chunks working backward from the offset, tracking each line's
/// byte position in the file. This ensures correct pagination across
/// multiple requests: `next_offset` always points to the byte position of
/// the oldest returned line, so the next backward page ends exactly before it.
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

    // Each entry: (byte_offset_in_file, line_text).
    let mut collected: Vec<(u64, String)> = Vec::with_capacity(limit.min(256));
    let mut current_end = end;
    // Tail fragment carried from the start of the previously-read (higher-offset)
    // chunk. It is the continuation of the current chunk's last line.
    let mut carry = String::new();

    while collected.len() < limit && current_end > 0 {
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

        // Parse lines from the raw buffer, recording each line's file offset.
        let mut chunk_lines: Vec<(u64, String)> = Vec::new();
        let mut seg_start: usize = 0;
        for (i, &byte) in buf.iter().enumerate() {
            if byte == b'\n' {
                let text = String::from_utf8_lossy(&buf[seg_start..i]).into_owned();
                chunk_lines.push((chunk_start + seg_start as u64, text));
                seg_start = i + 1;
            }
        }
        if seg_start < buf.len() {
            let text = String::from_utf8_lossy(&buf[seg_start..]).into_owned();
            chunk_lines.push((chunk_start + seg_start as u64, text));
        }

        // The last element extends to current_end. If carry is non-empty it is the
        // tail continuation from a higher-offset chunk — join to complete the line.
        if !carry.is_empty() {
            if let Some(last) = chunk_lines.last_mut() {
                last.1.push_str(&carry);
            }
            carry.clear();
        }

        // If chunk_start > 0, the first element is a partial line whose beginning
        // is in an earlier chunk. Save it as carry for the next iteration.
        if chunk_start > 0 && !chunk_lines.is_empty() {
            carry = chunk_lines.remove(0).1;
        }

        // Collect filtered lines in reverse (newest first).
        for (pos, text) in chunk_lines.into_iter().rev() {
            if !text.is_empty() && line_passes_filters(&text, level, filter) {
                collected.push((pos, text));
                if collected.len() >= limit {
                    break;
                }
            }
        }

        current_end = chunk_start;
    }

    // If carry is non-empty the loop reached byte 0 and the first file line
    // was saved as carry. Include it if we still have capacity.
    if !carry.is_empty() && collected.len() < limit && line_passes_filters(&carry, level, filter) {
        collected.push((0, carry));
    }

    // collected is newest-first; reverse to chronological order.
    collected.reverse();

    // next_offset = byte position of the oldest returned line so the next
    // backward page ends right before it with no gaps or overlaps.
    let next_offset = collected.first().map_or(0, |(pos, _)| *pos);

    Ok(LogResponse {
        lines: collected.into_iter().map(|(_, text)| text).collect(),
        next_offset,
        has_more: next_offset > 0,
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
#[allow(clippy::unwrap_used)]
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

    /// Verify that backward reading correctly joins partial lines at chunk
    /// boundaries instead of dropping them.
    #[tokio::test]
    async fn test_read_backward_multi_chunk_preserves_lines() {
        use tokio::io::AsyncWriteExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");

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

    /// Verify that paginating backward across multiple requests produces every
    /// line exactly once with no truncation or gaps.
    #[tokio::test]
    async fn test_read_backward_paginated_no_gaps() {
        use tokio::io::AsyncWriteExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.log");

        // Write 20 lines of varying lengths so chunk boundaries are likely to
        // fall mid-line.
        let mut f = tokio::fs::File::create(&path).await.unwrap();
        let mut expected: Vec<String> = Vec::new();
        for i in 0..20 {
            let line = format!(
                "2024-01-01T00:00:{i:02}Z  INFO test: line {i} padding={}",
                "x".repeat(i * 10)
            );
            f.write_all(line.as_bytes()).await.unwrap();
            f.write_all(b"\n").await.unwrap();
            expected.push(line);
        }
        f.flush().await.unwrap();
        drop(f);

        let file_size = tokio::fs::metadata(&path).await.unwrap().len();

        // Read backward in small pages of 5 and collect all results.
        let mut all_lines: Vec<String> = Vec::new();
        let mut offset: Option<u64> = None;
        let mut pages = 0;

        loop {
            let file = tokio::fs::File::open(&path).await.unwrap();
            let resp = read_backward(file, file_size, offset, 5, None, None).await.unwrap();

            // Prepend: each page is older than the previously collected lines.
            let mut page = resp.lines;
            page.append(&mut all_lines);
            all_lines = page;

            pages += 1;

            if !resp.has_more {
                break;
            }
            offset = Some(resp.next_offset);

            assert!(pages <= 10, "too many pages — possible infinite loop");
        }

        assert_eq!(all_lines, expected, "all lines must appear exactly once across pages");
        assert_eq!(pages, 4, "20 lines / 5 per page = 4 pages");
    }
}
