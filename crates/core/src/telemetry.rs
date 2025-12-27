// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Telemetry event emission and tracking.
//!
//! This module provides types and utilities for emitting structured telemetry events
//! from nodes. Telemetry events are used for observability, debugging, and UI timelines.
//!
//! ## Design Principles
//!
//! - **Best-effort delivery**: Telemetry never blocks audio processing
//! - **Wire-compatible**: Events wrap `CustomPacketData` for future "telemetry as track" support
//! - **Rate-limited**: Emitters automatically throttle high-frequency events
//! - **Drop accounting**: Track dropped events for health monitoring
//!
//! ## Event Type Convention
//!
//! All telemetry uses a single envelope type_id with `event_type` in the payload:
//!
//! ```text
//! type_id: core::telemetry/event@1
//! data: { event_type: "vad.start" | "stt.result" | "llm.response", ... }
//! ```
//!
//! ## Usage Example
//!
//! ```ignore
//! use streamkit_core::telemetry::TelemetryEmitter;
//!
//! // In node initialization
//! let telemetry = TelemetryEmitter::new(
//!     "my_node".to_string(),
//!     context.session_id.clone(),
//!     context.telemetry_tx.clone(),
//! );
//!
//! // Emit a simple event
//! telemetry.emit("processing.start", serde_json::json!({ "input_size": 1024 }));
//!
//! // Emit a correlated event (for grouping in UI)
//! telemetry.emit_with_correlation(
//!     "llm.response",
//!     "turn-abc123",
//!     serde_json::json!({ "latency_ms": 842, "output_chars": 456 })
//! );
//! ```

use crate::types::{CustomEncoding, CustomPacketData, PacketMetadata};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;
use ts_rs::TS;

/// The standard type_id for all telemetry events.
pub const TELEMETRY_TYPE_ID: &str = "core::telemetry/event@1";

/// A telemetry event emitted by a node.
///
/// This wraps `CustomPacketData` to maintain wire-compatibility with the packet system,
/// enabling future "telemetry as track" patterns where events flow through the graph.
///
/// ## Field Locations
///
/// - `session_id`, `node_id`: Envelope fields (not duplicated in packet data)
/// - `event_type`, `correlation_id`, `turn_id`: Inside `packet.data`
/// - `timestamp_us`: Inside `packet.metadata`
#[derive(Debug, Clone)]
pub struct TelemetryEvent {
    /// Session this event belongs to (for future shared bus / cross-session sinks)
    pub session_id: Option<String>,
    /// The node that emitted this event (canonical source, not in packet data)
    pub node_id: String,
    /// The telemetry payload wrapped as a Custom packet for wire compatibility
    pub packet: CustomPacketData,
}

impl TelemetryEvent {
    /// Create a new telemetry event.
    ///
    /// The `event_data` should contain at minimum `event_type`. Additional fields
    /// like `correlation_id`, `turn_id`, and event-specific data can be included.
    pub fn new(
        session_id: Option<String>,
        node_id: String,
        event_data: JsonValue,
        timestamp_us: u64,
    ) -> Self {
        Self {
            session_id,
            node_id,
            packet: CustomPacketData {
                type_id: TELEMETRY_TYPE_ID.to_string(),
                encoding: CustomEncoding::Json,
                data: event_data,
                metadata: Some(PacketMetadata {
                    timestamp_us: Some(timestamp_us),
                    duration_us: None,
                    sequence: None,
                }),
            },
        }
    }

    /// Extract the event_type from the packet data.
    pub fn event_type(&self) -> Option<&str> {
        self.packet.data.get("event_type").and_then(|v| v.as_str())
    }

    /// Extract the correlation_id from the packet data.
    pub fn correlation_id(&self) -> Option<&str> {
        self.packet.data.get("correlation_id").and_then(|v| v.as_str())
    }

    /// Extract the turn_id from the packet data.
    pub fn turn_id(&self) -> Option<&str> {
        self.packet.data.get("turn_id").and_then(|v| v.as_str())
    }

    /// Get the timestamp in microseconds.
    pub fn timestamp_us(&self) -> Option<u64> {
        self.packet.metadata.as_ref().and_then(|m| m.timestamp_us)
    }
}

/// Configuration for telemetry behavior.
///
/// These settings control buffering, redaction, and rate limiting.
/// Server-side redaction is applied before forwarding to WebSocket clients.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelemetryConfig {
    /// Whether telemetry is enabled for this session
    pub enabled: bool,
    /// Maximum number of events to buffer for backlog replay (default: 100)
    pub buffer_size: usize,
    /// Whether to redact text content before forwarding to clients
    pub redact_text: bool,
    /// Maximum characters to include in text fields (truncated beyond this)
    pub max_text_chars: usize,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self { enabled: true, buffer_size: 100, redact_text: false, max_text_chars: 100 }
    }
}

/// Helper for emitting telemetry events from nodes.
///
/// Provides best-effort, non-blocking emission with automatic rate limiting
/// and drop accounting. Uses `try_send()` to never block audio processing.
///
/// ## Drop Accounting
///
/// Dropped events are tracked and can be reported via health telemetry.
/// Call `emit_health()` periodically to report dropped event counts.
pub struct TelemetryEmitter {
    node_id: String,
    session_id: Option<String>,
    tx: Option<mpsc::Sender<TelemetryEvent>>,
    /// Events dropped because channel was full
    dropped_full: AtomicU64,
    /// Events dropped due to rate limiting
    dropped_rate_limit: AtomicU64,
    /// Last health emission time for throttling
    last_health_emit: Instant,
    /// Rate limiter state: (event_type_hash, last_emit_time, count_in_window)
    rate_limit_state: std::sync::Mutex<RateLimitState>,
}

/// Internal rate limiting state
struct RateLimitState {
    /// Per-event-type tracking: event_type -> (last_emit_instant, count_in_window)
    per_type: std::collections::HashMap<String, (Instant, u32)>,
    /// Window duration for rate limiting
    window: std::time::Duration,
    /// Max events per window per event_type
    max_per_window: u32,
}

impl Default for RateLimitState {
    fn default() -> Self {
        Self {
            per_type: std::collections::HashMap::new(),
            window: std::time::Duration::from_secs(1),
            max_per_window: 100, // 100 events/sec per event_type by default
        }
    }
}

impl TelemetryEmitter {
    /// Health emission interval (5 seconds)
    const HEALTH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

    /// Create a new telemetry emitter for a node.
    pub fn new(
        node_id: String,
        session_id: Option<String>,
        tx: Option<mpsc::Sender<TelemetryEvent>>,
    ) -> Self {
        Self {
            node_id,
            session_id,
            tx,
            dropped_full: AtomicU64::new(0),
            dropped_rate_limit: AtomicU64::new(0),
            last_health_emit: Instant::now(),
            rate_limit_state: std::sync::Mutex::new(RateLimitState::default()),
        }
    }

    /// Get current timestamp in microseconds since UNIX epoch.
    #[allow(clippy::cast_possible_truncation)] // u64 microseconds covers ~500,000 years
    fn now_us() -> u64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
    }

    /// Check if an event should be rate-limited.
    #[allow(clippy::expect_used)] // Mutex poisoning indicates a serious bug, panic is appropriate
    fn should_rate_limit(&self, event_type: &str) -> bool {
        let mut state = self.rate_limit_state.lock().expect("rate limit mutex poisoned");
        let now = Instant::now();
        let window = state.window;
        let max_per_window = state.max_per_window;

        let entry = state.per_type.entry(event_type.to_string()).or_insert((now, 0));

        // Reset window if expired
        if now.duration_since(entry.0) >= window {
            entry.0 = now;
            entry.1 = 1;
            drop(state);
            return false;
        }

        // Check if we're over the limit
        if entry.1 >= max_per_window {
            drop(state);
            return true;
        }

        entry.1 += 1;
        drop(state);
        false
    }

    /// Best-effort emit a telemetry event. Never blocks.
    ///
    /// Returns `true` if the event was sent (or queued), `false` if dropped.
    pub fn emit(&self, event_type: &str, data: JsonValue) -> bool {
        self.emit_internal(event_type, None, None, data)
    }

    /// Emit an event with a correlation ID for grouping related events.
    pub fn emit_with_correlation(
        &self,
        event_type: &str,
        correlation_id: &str,
        data: JsonValue,
    ) -> bool {
        self.emit_internal(event_type, Some(correlation_id), None, data)
    }

    /// Emit an event with a turn ID for voice agent conversation grouping.
    pub fn emit_with_turn(&self, event_type: &str, turn_id: &str, data: JsonValue) -> bool {
        self.emit_internal(event_type, None, Some(turn_id), data)
    }

    /// Emit an event with both correlation and turn IDs.
    pub fn emit_correlated(
        &self,
        event_type: &str,
        correlation_id: &str,
        turn_id: &str,
        data: JsonValue,
    ) -> bool {
        self.emit_internal(event_type, Some(correlation_id), Some(turn_id), data)
    }

    /// Internal emit implementation.
    fn emit_internal(
        &self,
        event_type: &str,
        correlation_id: Option<&str>,
        turn_id: Option<&str>,
        mut data: JsonValue,
    ) -> bool {
        let Some(ref tx) = self.tx else {
            return false;
        };

        // Rate limiting check
        if self.should_rate_limit(event_type) {
            self.dropped_rate_limit.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        // Ensure data is an object and add standard fields
        if let Some(obj) = data.as_object_mut() {
            obj.insert("event_type".to_string(), JsonValue::String(event_type.to_string()));
            if let Some(cid) = correlation_id {
                obj.insert("correlation_id".to_string(), JsonValue::String(cid.to_string()));
            }
            if let Some(tid) = turn_id {
                obj.insert("turn_id".to_string(), JsonValue::String(tid.to_string()));
            }
        } else {
            // Wrap non-object data
            data = serde_json::json!({
                "event_type": event_type,
                "correlation_id": correlation_id,
                "turn_id": turn_id,
                "value": data,
            });
        }

        let event = TelemetryEvent::new(
            self.session_id.clone(),
            self.node_id.clone(),
            data,
            Self::now_us(),
        );

        // Best-effort send - never block
        match tx.try_send(event) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped_full.fetch_add(1, Ordering::Relaxed);
                false
            },
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Get the current dropped event counts.
    pub fn dropped_counts(&self) -> (u64, u64) {
        (self.dropped_full.load(Ordering::Relaxed), self.dropped_rate_limit.load(Ordering::Relaxed))
    }

    /// Emit a health event if the interval has passed or if there are dropped events.
    ///
    /// Returns `true` if a health event was emitted.
    pub fn maybe_emit_health(&mut self) -> bool {
        let (dropped_full, dropped_rate_limit) = self.dropped_counts();
        let has_drops = dropped_full > 0 || dropped_rate_limit > 0;
        let interval_passed = self.last_health_emit.elapsed() >= Self::HEALTH_INTERVAL;

        if !has_drops && !interval_passed {
            return false;
        }

        if has_drops || interval_passed {
            self.last_health_emit = Instant::now();

            // Only emit if there's something to report
            if has_drops {
                let emitted = self.emit(
                    "telemetry.health",
                    serde_json::json!({
                        "dropped_due_to_full": dropped_full,
                        "dropped_due_to_rate_limit": dropped_rate_limit,
                    }),
                );

                // Reset counters after emission
                if emitted {
                    self.dropped_full.store(0, Ordering::Relaxed);
                    self.dropped_rate_limit.store(0, Ordering::Relaxed);
                }

                return emitted;
            }
        }

        false
    }

    /// Configure rate limiting for this emitter.
    ///
    /// # Panics
    ///
    /// Panics if the internal rate limit mutex is poisoned (indicates a prior panic).
    #[allow(clippy::expect_used)] // Mutex poisoning indicates a serious bug, panic is appropriate
    pub fn set_rate_limit(&self, max_per_second: u32) {
        let mut state = self.rate_limit_state.lock().expect("rate limit mutex poisoned");
        state.max_per_window = max_per_second;
        state.window = std::time::Duration::from_secs(1);
    }
}

/// Helper functions for emitting telemetry events directly from a sender.
/// These are lower-level functions for cases where you don't want to use `TelemetryEmitter`.
pub mod telemetry_helpers {
    use super::TelemetryEvent;
    use serde_json::Value as JsonValue;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::mpsc;

    /// Emit a simple telemetry event.
    #[allow(clippy::cast_possible_truncation)] // u64 microseconds covers ~500,000 years
    pub fn emit(
        tx: &mpsc::Sender<TelemetryEvent>,
        session_id: Option<String>,
        node_id: &str,
        event_type: &str,
        data: &JsonValue,
    ) -> bool {
        let timestamp_us =
            SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0);

        let event_data = data.as_object().map_or_else(
            || {
                serde_json::json!({
                    "event_type": event_type,
                    "value": data,
                })
            },
            |obj| {
                let mut obj = obj.clone();
                obj.insert("event_type".to_string(), JsonValue::String(event_type.to_string()));
                JsonValue::Object(obj)
            },
        );

        let event = TelemetryEvent::new(session_id, node_id.to_string(), event_data, timestamp_us);

        tx.try_send(event).is_ok()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::unreadable_literal)]
mod tests {
    use super::*;

    #[test]
    fn test_telemetry_event_creation() {
        let event = TelemetryEvent::new(
            Some("session-123".to_string()),
            "test-node".to_string(),
            serde_json::json!({
                "event_type": "test.event",
                "correlation_id": "corr-456",
            }),
            1234567890,
        );

        assert_eq!(event.session_id, Some("session-123".to_string()));
        assert_eq!(event.node_id, "test-node");
        assert_eq!(event.event_type(), Some("test.event"));
        assert_eq!(event.correlation_id(), Some("corr-456"));
        assert_eq!(event.timestamp_us(), Some(1234567890));
        assert_eq!(event.packet.type_id, TELEMETRY_TYPE_ID);
    }

    #[test]
    fn test_telemetry_config_default() {
        let config = TelemetryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.buffer_size, 100);
        assert!(!config.redact_text);
        assert_eq!(config.max_text_chars, 100);
    }

    #[tokio::test]
    async fn test_emitter_basic() {
        let (tx, mut rx) = mpsc::channel(10);
        let emitter =
            TelemetryEmitter::new("node-1".to_string(), Some("session-1".to_string()), Some(tx));

        assert!(emitter.emit("test.event", serde_json::json!({ "key": "value" })));

        let event = rx.recv().await.unwrap();
        assert_eq!(event.node_id, "node-1");
        assert_eq!(event.session_id, Some("session-1".to_string()));
        assert_eq!(event.event_type(), Some("test.event"));
        assert_eq!(event.packet.data.get("key").and_then(|v| v.as_str()), Some("value"));
    }

    #[tokio::test]
    async fn test_emitter_drop_accounting() {
        // Create a channel with capacity 1
        let (tx, _rx) = mpsc::channel(1);
        let emitter = TelemetryEmitter::new("node-1".to_string(), None, Some(tx));

        // First event should succeed
        assert!(emitter.emit("event1", serde_json::json!({})));

        // Second event should be dropped (channel full)
        assert!(!emitter.emit("event2", serde_json::json!({})));

        let (dropped_full, _) = emitter.dropped_counts();
        assert_eq!(dropped_full, 1);
    }

    #[test]
    fn test_emitter_no_tx() {
        let emitter = TelemetryEmitter::new("node-1".to_string(), None, None);

        // Should return false but not panic
        assert!(!emitter.emit("test.event", serde_json::json!({})));
    }
}
