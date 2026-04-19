// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allow println/eprintln in CLI client - these are for direct user output, not logging
#![allow(clippy::disallowed_macros)]

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::{cursor, execute, terminal};
use futures::StreamExt as FuturesStreamExt;
use serde::Serialize;
use streamkit_api::NodeStats;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use crate::client::control_ws_url;

/// Helper: connect to the control WebSocket, optionally with a Bearer token.
async fn connect_control_ws(
    server_url: &str,
    token: Option<&str>,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let ws_url = control_ws_url(server_url)?;
    let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(ws_url.as_str())
        .header("Host", ws_url.host_str().unwrap_or("localhost"))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        );
    if let Some(t) = token {
        request = request.header("Authorization", format!("Bearer {t}"));
    }
    let request = request.body(())?;
    let (ws_stream, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(ws_stream)
}

/// Snapshot of a node's stats used for rate calculation.
#[derive(Debug, Clone)]
struct StatsSnapshot {
    stats: NodeStats,
    observed_at: Instant,
}

/// Per-second rates derived from two consecutive snapshots.
#[derive(Debug, Clone, Serialize)]
pub struct NodeRates {
    pub recv_per_sec: f64,
    pub sent_per_sec: f64,
    pub drop_per_sec: f64,
    pub err_per_sec: f64,
}

/// Calculate per-second rates between two stats snapshots.
#[allow(clippy::cast_precision_loss)]
pub fn calculate_rates(prev: &NodeStats, curr: &NodeStats, elapsed_secs: f64) -> NodeRates {
    if elapsed_secs <= 0.0 {
        return NodeRates {
            recv_per_sec: 0.0,
            sent_per_sec: 0.0,
            drop_per_sec: 0.0,
            err_per_sec: 0.0,
        };
    }
    NodeRates {
        recv_per_sec: (curr.received.saturating_sub(prev.received)) as f64 / elapsed_secs,
        sent_per_sec: (curr.sent.saturating_sub(prev.sent)) as f64 / elapsed_secs,
        drop_per_sec: (curr.discarded.saturating_sub(prev.discarded)) as f64 / elapsed_secs,
        err_per_sec: (curr.errored.saturating_sub(prev.errored)) as f64 / elapsed_secs,
    }
}

/// JSON output for a single node's live stats update (used by `top --json`).
#[derive(Serialize)]
struct TopJsonEntry {
    node_id: String,
    rates: NodeRates,
    total_received: u64,
    total_sent: u64,
    total_discarded: u64,
    total_errored: u64,
    duration_secs: f64,
}

/// Collect one round of stats from all nodes and print a snapshot table.
///
/// Connects to the control WebSocket, filters `NodeStatsUpdated` events for the
/// given session (or all sessions when `session_id` is `None`), waits until no
/// new updates arrive for `quiet_period` seconds (or until `timeout` elapses),
/// then prints and exits.
///
/// # Errors
///
/// Returns an error if the WebSocket connection fails or message parsing fails.
pub async fn run_stats(
    session_id: Option<&str>,
    server_url: &str,
    json: bool,
    timeout_secs: u64,
    token: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(?session_id, timeout_secs, "Collecting stats snapshot");

    let mut ws_stream = connect_control_ws(server_url, token).await?;

    let mut stats_map: BTreeMap<String, NodeStats> = BTreeMap::new();
    let quiet_period = tokio::time::Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_secs);
    let mut last_update = tokio::time::Instant::now();

    debug!(timeout_secs, "Waiting for stats events");

    loop {
        let now = tokio::time::Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline - now;

        let since_last = now - last_update;
        let quiet_remaining = quiet_period.saturating_sub(since_last);

        let wait_dur = remaining.min(quiet_remaining);

        tokio::select! {
            () = tokio::time::sleep(wait_dur) => {
                let since_last = tokio::time::Instant::now() - last_update;
                if since_last >= quiet_period && !stats_map.is_empty() {
                    break;
                }
            }
            msg = FuturesStreamExt::next(&mut ws_stream) => {
                let Some(msg) = msg else { break; };
                let msg = msg?;
                let Message::Text(text) = msg else { continue; };

                if let Some((sid, node_id, stats)) = parse_stats_event_global(&text, session_id) {
                    let key = if session_id.is_some() {
                        node_id
                    } else {
                        format!("{sid}/{node_id}")
                    };
                    debug!(key = %key, "Received stats update");
                    stats_map.insert(key, stats);
                    last_update = tokio::time::Instant::now();
                }
            }
        }
    }

    if let Err(e) = ws_stream.close(None).await {
        warn!(error = %e, "Failed to close WebSocket cleanly");
    }

    if stats_map.is_empty() {
        let target = session_id.unwrap_or("all sessions");
        eprintln!(
            "No stats received for {target} (timed out after {timeout_secs}s). \
             The session may be idle or may not exist."
        );
        return Ok(());
    }

    if json {
        let entries: Vec<serde_json::Value> = stats_map
            .iter()
            .map(|(node_id, stats)| {
                serde_json::json!({
                    "node_id": node_id,
                    "stats": {
                        "received": stats.received,
                        "sent": stats.sent,
                        "discarded": stats.discarded,
                        "errored": stats.errored,
                        "duration_secs": stats.duration_secs,
                    }
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        let header = session_id.unwrap_or("All Sessions");
        print_stats_table(header, &stats_map);
    }

    Ok(())
}

/// Format a row of stats values into columns for the stats table.
fn format_stats_row(node_id: &str, max_id_len: usize, cols: &[String; 5]) -> String {
    format!(
        "{:<width$}{:>10}{:>10}{:>12}{:>10}{:>12}",
        truncate_id(node_id, max_id_len),
        cols[0],
        cols[1],
        cols[2],
        cols[3],
        cols[4],
        width = max_id_len + 1,
    )
}

fn print_stats_table(header: &str, stats_map: &BTreeMap<String, NodeStats>) {
    let id_width = 14;
    let divider_len = id_width + 1 + 10 + 10 + 12 + 10 + 12;
    println!("Session: {header}");
    println!(
        "{:<width$}{:>10}{:>10}{:>12}{:>10}{:>12}",
        "Node",
        "Received",
        "Sent",
        "Discarded",
        "Errored",
        "Duration",
        width = id_width + 1,
    );
    println!("{}", "─".repeat(divider_len));
    for (node_id, stats) in stats_map {
        println!(
            "{}",
            format_stats_row(
                node_id,
                id_width,
                &[
                    stats.received.to_string(),
                    stats.sent.to_string(),
                    stats.discarded.to_string(),
                    stats.errored.to_string(),
                    format!("{:.1}s", stats.duration_secs),
                ],
            )
        );
    }
}

/// RAII guard that restores terminal state on drop.
///
/// Ensures `disable_raw_mode` + cursor-show even on panic or early return.
struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        terminal::enable_raw_mode()?;
        execute!(
            io::stdout(),
            cursor::Hide,
            terminal::Clear(terminal::ClearType::All),
            cursor::MoveTo(0, 0)
        )?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let _ = execute!(io::stdout(), cursor::Show);
        // Print a newline so the shell prompt appears cleanly
        let _ = writeln!(io::stdout());
    }
}

/// Live-updating dashboard of session stats (like Unix `top`).
///
/// Subscribes to `NodeStatsUpdated` events and re-renders the table in place
/// every time an update arrives or every second (whichever comes first).
/// Uses basic `crossterm` terminal control for cursor positioning and clearing,
/// with an RAII guard to ensure terminal state is always restored.
///
/// When `session_id` is `None`, shows stats from all sessions keyed as
/// `"session_id/node_id"`.
///
/// # Errors
///
/// Returns an error if the WebSocket connection fails, terminal control fails,
/// or message parsing fails.
pub async fn run_top(
    session_id: Option<&str>,
    server_url: &str,
    json: bool,
    token: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(?session_id, "Starting live stats dashboard");

    let mut ws_stream = connect_control_ws(server_url, token).await?;

    let mut snapshots: BTreeMap<String, StatsSnapshot> = BTreeMap::new();
    let mut prev_snapshots: BTreeMap<String, StatsSnapshot> = BTreeMap::new();
    let start = Instant::now();

    // RAII guard ensures terminal state is always restored, even on panic
    let _guard = if json { None } else { Some(RawModeGuard::new()?) };

    let result = run_top_loop(
        session_id,
        &mut ws_stream,
        &mut snapshots,
        &mut prev_snapshots,
        start,
        json,
    )
    .await;

    // _guard dropped here → terminal restored

    if let Err(e) = ws_stream.close(None).await {
        warn!(error = %e, "Failed to close WebSocket cleanly");
    }

    result
}

async fn run_top_loop(
    session_id: Option<&str>,
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    snapshots: &mut BTreeMap<String, StatsSnapshot>,
    prev_snapshots: &mut BTreeMap<String, StatsSnapshot>,
    start: Instant,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    // Don't pile up missed ticks — just skip them
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Use crossterm event stream for key input (works in raw mode, unlike tokio::signal)
    let mut key_reader = crossterm::event::EventStream::new();

    let header = session_id.unwrap_or("All Sessions");

    loop {
        tokio::select! {
            // Periodic re-render: keeps uptime ticking even when idle
            _ = ticker.tick() => {
                if !json && !snapshots.is_empty() {
                    render_top_table(header, snapshots, prev_snapshots, start)?;
                }
            }
            // Keyboard input: q or Ctrl-C to quit
            key_event = FuturesStreamExt::next(&mut key_reader) => {
                if let Some(Ok(crossterm::event::Event::Key(key))) = key_event {
                    use crossterm::event::{KeyCode, KeyModifiers};
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('q'), _)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            debug!("Quit key received, exiting top");
                            break;
                        }
                        _ => {}
                    }
                }
            }
            // WebSocket messages
            msg = FuturesStreamExt::next(ws_stream) => {
                let Some(msg) = msg else { break; };
                let msg = msg?;
                let Message::Text(text) = msg else { continue; };

                if let Some((sid, node_id, stats)) = parse_stats_event_global(&text, session_id) {
                    let now = Instant::now();
                    let key = if session_id.is_some() {
                        node_id
                    } else {
                        format!("{sid}/{node_id}")
                    };
                    debug!(key = %key, "Stats tick");

                    // Rotate current → previous
                    if let Some(current) = snapshots.get(&key) {
                        prev_snapshots.insert(key.clone(), current.clone());
                    }
                    snapshots.insert(key, StatsSnapshot { stats, observed_at: now });

                    if json {
                        render_top_json(snapshots, prev_snapshots);
                    } else {
                        render_top_table(header, snapshots, prev_snapshots, start)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn render_top_table(
    header: &str,
    snapshots: &BTreeMap<String, StatsSnapshot>,
    prev_snapshots: &BTreeMap<String, StatsSnapshot>,
    start: Instant,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut stdout = io::stdout();
    execute!(stdout, cursor::MoveTo(0, 0), terminal::Clear(terminal::ClearType::All))?;

    let uptime = start.elapsed();
    let hours = uptime.as_secs() / 3600;
    let minutes = (uptime.as_secs() % 3600) / 60;
    let seconds = uptime.as_secs() % 60;

    let id_width = 14;
    let divider_len = id_width + 1 + 10 + 10 + 10 + 10 + 12;

    writeln!(
        stdout,
        "Session: {header}                    Uptime: {hours:02}:{minutes:02}:{seconds:02}"
    )?;
    writeln!(stdout, "{}", "─".repeat(divider_len))?;
    writeln!(
        stdout,
        "{:<width$}{:>10}{:>10}{:>10}{:>10}{:>12}",
        "Node",
        "Recv/s",
        "Sent/s",
        "Drop/s",
        "Err/s",
        "Total Recv",
        width = id_width + 1,
    )?;

    for (node_id, snap) in snapshots {
        let rates = prev_snapshots.get(node_id).map_or(
            NodeRates { recv_per_sec: 0.0, sent_per_sec: 0.0, drop_per_sec: 0.0, err_per_sec: 0.0 },
            |prev| {
                let elapsed = snap.observed_at.duration_since(prev.observed_at).as_secs_f64();
                calculate_rates(&prev.stats, &snap.stats, elapsed)
            },
        );

        writeln!(
            stdout,
            "{:<width$}{:>10.1}{:>10.1}{:>10.1}{:>10.1}{:>12}",
            truncate_id(node_id, id_width),
            rates.recv_per_sec,
            rates.sent_per_sec,
            rates.drop_per_sec,
            rates.err_per_sec,
            snap.stats.received,
            width = id_width + 1,
        )?;
    }

    writeln!(stdout, "{}", "─".repeat(divider_len))?;
    writeln!(stdout, "Press q to quit")?;
    stdout.flush()?;

    Ok(())
}

fn render_top_json(
    snapshots: &BTreeMap<String, StatsSnapshot>,
    prev_snapshots: &BTreeMap<String, StatsSnapshot>,
) {
    let entries: Vec<TopJsonEntry> = snapshots
        .iter()
        .map(|(node_id, snap)| {
            let rates = prev_snapshots.get(node_id).map_or(
                NodeRates {
                    recv_per_sec: 0.0,
                    sent_per_sec: 0.0,
                    drop_per_sec: 0.0,
                    err_per_sec: 0.0,
                },
                |prev| {
                    let elapsed = snap.observed_at.duration_since(prev.observed_at).as_secs_f64();
                    calculate_rates(&prev.stats, &snap.stats, elapsed)
                },
            );
            TopJsonEntry {
                node_id: node_id.clone(),
                rates,
                total_received: snap.stats.received,
                total_sent: snap.stats.sent,
                total_discarded: snap.stats.discarded,
                total_errored: snap.stats.errored,
                duration_secs: snap.stats.duration_secs,
            }
        })
        .collect();

    match serde_json::to_string(&entries) {
        Ok(json) => println!("{json}"),
        Err(e) => warn!(error = %e, "Failed to serialize top JSON output"),
    }
}

/// Parse a WebSocket message as a `NodeStatsUpdated` event, optionally filtering
/// by session. Returns `(session_id, node_id, stats)` on match.
fn parse_stats_event_global(
    text: &str,
    session_filter: Option<&str>,
) -> Option<(String, String, NodeStats)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("type")?.as_str()? != "event" {
        return None;
    }
    let payload = v.get("payload")?;
    if payload.get("event")?.as_str()? != "nodestatsupdated" {
        return None;
    }
    let sid = payload.get("session_id")?.as_str()?.to_string();
    if let Some(filter) = session_filter {
        if sid != filter {
            return None;
        }
    }
    let node_id = payload.get("node_id")?.as_str()?.to_string();
    let stats: NodeStats = serde_json::from_value(payload.get("stats")?.clone()).ok()?;
    Some((sid, node_id, stats))
}

/// Truncate a string to `max_len` display characters, appending `…` if truncated.
///
/// Uses `chars()` iteration to avoid panicking on non-ASCII boundaries.
fn truncate_id(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_len - 1).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_rates_basic() {
        let prev =
            NodeStats { received: 100, sent: 90, discarded: 5, errored: 2, duration_secs: 10.0 };
        let curr =
            NodeStats { received: 200, sent: 180, discarded: 10, errored: 4, duration_secs: 20.0 };
        let rates = calculate_rates(&prev, &curr, 10.0);
        assert!((rates.recv_per_sec - 10.0).abs() < f64::EPSILON);
        assert!((rates.sent_per_sec - 9.0).abs() < f64::EPSILON);
        assert!((rates.drop_per_sec - 0.5).abs() < f64::EPSILON);
        assert!((rates.err_per_sec - 0.2).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_rates_zero_elapsed() {
        let prev =
            NodeStats { received: 100, sent: 90, discarded: 5, errored: 2, duration_secs: 10.0 };
        let curr =
            NodeStats { received: 200, sent: 180, discarded: 10, errored: 4, duration_secs: 10.0 };
        let rates = calculate_rates(&prev, &curr, 0.0);
        assert!((rates.recv_per_sec).abs() < f64::EPSILON);
        assert!((rates.sent_per_sec).abs() < f64::EPSILON);
        assert!((rates.drop_per_sec).abs() < f64::EPSILON);
        assert!((rates.err_per_sec).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_rates_no_change() {
        let stats =
            NodeStats { received: 100, sent: 90, discarded: 5, errored: 2, duration_secs: 10.0 };
        let rates = calculate_rates(&stats, &stats, 5.0);
        assert!((rates.recv_per_sec).abs() < f64::EPSILON);
        assert!((rates.sent_per_sec).abs() < f64::EPSILON);
        assert!((rates.drop_per_sec).abs() < f64::EPSILON);
        assert!((rates.err_per_sec).abs() < f64::EPSILON);
    }

    #[test]
    fn test_calculate_rates_saturating_sub() {
        // Curr < prev (counter reset scenario) — should not panic or go negative.
        let prev =
            NodeStats { received: 200, sent: 180, discarded: 10, errored: 4, duration_secs: 20.0 };
        let curr =
            NodeStats { received: 50, sent: 40, discarded: 1, errored: 0, duration_secs: 5.0 };
        let rates = calculate_rates(&prev, &curr, 5.0);
        assert!(rates.recv_per_sec >= 0.0);
        assert!(rates.sent_per_sec >= 0.0);
        assert!(rates.drop_per_sec >= 0.0);
        assert!(rates.err_per_sec >= 0.0);
    }

    #[test]
    fn test_truncate_id_short() {
        assert_eq!(truncate_id("abc", 13), "abc");
    }

    #[test]
    fn test_truncate_id_exact() {
        let s = "a".repeat(13);
        assert_eq!(truncate_id(&s, 13), s);
    }

    #[test]
    fn test_truncate_id_long() {
        let s = "a".repeat(20);
        let result = truncate_id(&s, 13);
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), 13);
    }

    #[test]
    fn test_truncate_id_non_ascii() {
        // Non-ASCII characters must not panic
        let s = "日本語のテストノード名前";
        let result = truncate_id(s, 5);
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), 5);
    }

    #[test]
    fn test_truncate_id_emoji() {
        let s = "🎵🎶🔊🎤🎧🎼";
        let result = truncate_id(s, 4);
        assert!(result.ends_with('…'));
        assert_eq!(result.chars().count(), 4);
    }

    #[test]
    fn test_parse_stats_event_global_specific_session() {
        let msg = r#"{"type":"event","payload":{"event":"nodestatsupdated","session_id":"abc","node_id":"src","stats":{"received":10,"sent":10,"discarded":0,"errored":0,"duration_secs":1.0}}}"#;
        let result = parse_stats_event_global(msg, Some("abc"));
        assert!(result.is_some());
        let (sid, nid, _) = result.unwrap();
        assert_eq!(sid, "abc");
        assert_eq!(nid, "src");
        assert!(parse_stats_event_global(msg, Some("xyz")).is_none());
    }

    #[test]
    fn test_parse_stats_event_global_all_sessions() {
        let msg = r#"{"type":"event","payload":{"event":"nodestatsupdated","session_id":"abc","node_id":"src","stats":{"received":10,"sent":10,"discarded":0,"errored":0,"duration_secs":1.0}}}"#;
        let result = parse_stats_event_global(msg, None);
        assert!(result.is_some());
        let (sid, nid, _) = result.unwrap();
        assert_eq!(sid, "abc");
        assert_eq!(nid, "src");
    }
}
