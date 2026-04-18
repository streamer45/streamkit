// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allow println/eprintln in CLI client - these are for direct user output, not logging
#![allow(clippy::disallowed_macros)]

use std::collections::BTreeMap;
use std::io::{self, Write};
use std::time::Instant;

use crossterm::{cursor, execute, terminal};
use futures::StreamExt as FuturesStreamExt;
use serde::Serialize;
use streamkit_api::NodeStats;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info};

use crate::client::control_ws_url;

/// Helper: connect to the control WebSocket and return the stream.
async fn connect_control_ws(
    server_url: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Box<dyn std::error::Error + Send + Sync>,
> {
    let ws_url = control_ws_url(server_url)?.to_string();
    let (ws_stream, _) = tokio_tungstenite::connect_async(ws_url).await?;
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
/// given session, waits until no new updates arrive for `quiet_period` seconds
/// (or until `timeout` elapses), then prints and exits.
///
/// # Errors
///
/// Returns an error if the WebSocket connection fails or message parsing fails.
pub async fn run_stats(
    session_id: &str,
    server_url: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(session_id, "Collecting stats snapshot");

    let mut ws_stream = connect_control_ws(server_url).await?;

    let mut stats_map: BTreeMap<String, NodeStats> = BTreeMap::new();
    let quiet_period = tokio::time::Duration::from_millis(500);
    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
    let mut last_update = tokio::time::Instant::now();

    debug!("Waiting for stats events (timeout=5s, quiet=500ms)");

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

                if let Some((node_id, stats)) = parse_stats_event(&text, session_id) {
                    debug!(node_id = %node_id, "Received stats update");
                    stats_map.insert(node_id, stats);
                    last_update = tokio::time::Instant::now();
                }
            }
        }
    }

    drop(ws_stream.close(None).await);

    if stats_map.is_empty() {
        eprintln!("No stats received for session '{session_id}' (timed out after 5s)");
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
        print_stats_table(session_id, &stats_map);
    }

    Ok(())
}

fn print_stats_table(session_id: &str, stats_map: &BTreeMap<String, NodeStats>) {
    println!("Session: {session_id}");
    println!(
        "{:<14}{:>10}{:>10}{:>12}{:>10}{:>12}",
        "Node", "Received", "Sent", "Discarded", "Errored", "Duration"
    );
    println!("{}", "─".repeat(68));
    for (node_id, stats) in stats_map {
        println!(
            "{:<14}{:>10}{:>10}{:>12}{:>10}{:>11.1}s",
            truncate_id(node_id, 13),
            stats.received,
            stats.sent,
            stats.discarded,
            stats.errored,
            stats.duration_secs,
        );
    }
}

/// Live-updating dashboard of session stats (like Unix `top`).
///
/// Subscribes to `NodeStatsUpdated` events and re-renders the table in place
/// every time an update arrives. Uses basic `crossterm` terminal control for
/// cursor positioning and clearing.
///
/// # Errors
///
/// Returns an error if the WebSocket connection fails, terminal control fails,
/// or message parsing fails.
pub async fn run_top(
    session_id: &str,
    server_url: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!(session_id, "Starting live stats dashboard");

    let mut ws_stream = connect_control_ws(server_url).await?;

    let mut snapshots: BTreeMap<String, StatsSnapshot> = BTreeMap::new();
    let mut prev_snapshots: BTreeMap<String, StatsSnapshot> = BTreeMap::new();
    let start = Instant::now();

    if !json {
        terminal::enable_raw_mode()?;
        execute!(io::stdout(), terminal::Clear(terminal::ClearType::All), cursor::MoveTo(0, 0))?;
    }

    let result =
        run_top_loop(session_id, &mut ws_stream, &mut snapshots, &mut prev_snapshots, start, json)
            .await;

    if !json {
        terminal::disable_raw_mode()?;
        // Move cursor below the table so the shell prompt appears cleanly
        execute!(io::stdout(), cursor::Show)?;
        println!();
    }

    drop(ws_stream.close(None).await);
    result
}

async fn run_top_loop(
    session_id: &str,
    ws_stream: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    snapshots: &mut BTreeMap<String, StatsSnapshot>,
    prev_snapshots: &mut BTreeMap<String, StatsSnapshot>,
    start: Instant,
    json: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                debug!("Ctrl-C received, exiting top");
                break;
            }
            msg = FuturesStreamExt::next(ws_stream) => {
                let Some(msg) = msg else { break; };
                let msg = msg?;
                let Message::Text(text) = msg else { continue; };

                if let Some((node_id, stats)) = parse_stats_event(&text, session_id) {
                    let now = Instant::now();
                    debug!(node_id = %node_id, "Stats tick");

                    // Rotate current → previous
                    if let Some(current) = snapshots.get(&node_id) {
                        prev_snapshots.insert(node_id.clone(), current.clone());
                    }
                    snapshots.insert(node_id, StatsSnapshot { stats, observed_at: now });

                    if json {
                        render_top_json(snapshots, prev_snapshots);
                    } else {
                        render_top_table(session_id, snapshots, prev_snapshots, start)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn render_top_table(
    session_id: &str,
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

    writeln!(
        stdout,
        "Session: {session_id}                    Uptime: {hours:02}:{minutes:02}:{seconds:02}"
    )?;
    writeln!(stdout, "{}", "─".repeat(68))?;
    writeln!(
        stdout,
        "{:<14}{:>10}{:>10}{:>10}{:>10}{:>12}",
        "Node", "Recv/s", "Sent/s", "Drop/s", "Err/s", "Total Recv"
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
            "{:<14}{:>10.1}{:>10.1}{:>10.1}{:>10.1}{:>12}",
            truncate_id(node_id, 13),
            rates.recv_per_sec,
            rates.sent_per_sec,
            rates.drop_per_sec,
            rates.err_per_sec,
            snap.stats.received,
        )?;
    }

    writeln!(stdout, "{}", "─".repeat(68))?;
    writeln!(stdout, "Press Ctrl-C to quit")?;
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

    if let Ok(json) = serde_json::to_string(&entries) {
        println!("{json}");
    }
}

/// Parse a WebSocket message as a `NodeStatsUpdated` event for the given session.
///
/// Returns `Some((node_id, stats))` if the message matches, `None` otherwise.
fn parse_stats_event(text: &str, session_id: &str) -> Option<(String, NodeStats)> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;

    // Must be an event message
    if v.get("type")?.as_str()? != "event" {
        return None;
    }

    let payload = v.get("payload")?;

    // Must be a nodestats event for the target session
    if payload.get("event")?.as_str()? != "nodestatsupdated" {
        return None;
    }
    if payload.get("session_id")?.as_str()? != session_id {
        return None;
    }

    let node_id = payload.get("node_id")?.as_str()?.to_string();
    let stats: NodeStats = serde_json::from_value(payload.get("stats")?.clone()).ok()?;

    Some((node_id, stats))
}

/// Truncate a string to `max_len` characters, appending `…` if truncated.
fn truncate_id(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
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
        assert_eq!(result.len(), 13 + "…".len() - 1);
        assert!(result.ends_with('…'));
    }
}
