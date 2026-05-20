// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! OpenTelemetry metrics for native plugin FFI calls.
//!
//! Instruments are built against the OTel global meter on first access.
//! The OTel meter provider must be initialized before the first plugin load.

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::KeyValue;

/// Outcome of an FFI call, used by [`PluginMetrics::record`].
#[derive(Clone, Copy, Debug)]
pub enum CallOutcome {
    Success,
    Error,
    /// FFI call panicked.  Only bumps `plugin.panics` (not `plugin.errors`)
    /// so dashboards can sum `errors + panics` without double-counting.
    Panic,
}

/// Per-plugin metrics for FFI call instrumentation.
///
/// A single global instance is shared across all plugin instances;
/// individual calls are distinguished by `plugin.kind` and `op` labels.
pub struct PluginMetrics {
    call_duration: Histogram<f64>,
    calls_total: Counter<u64>,
    errors_total: Counter<u64>,
    panics_total: Counter<u64>,
    timeouts_total: Counter<u64>,
}

impl PluginMetrics {
    pub fn new() -> Self {
        let meter = opentelemetry::global::meter("skit_plugin_native");
        Self {
            call_duration: meter
                .f64_histogram("plugin.call.duration")
                .with_description("Duration of native plugin FFI calls")
                .with_unit("s")
                .with_boundaries(
                    streamkit_core::metrics::HISTOGRAM_BOUNDARIES_NODE_EXECUTION.to_vec(),
                )
                .build(),
            calls_total: meter
                .u64_counter("plugin.calls")
                .with_description("Total native plugin FFI calls")
                .build(),
            errors_total: meter
                .u64_counter("plugin.errors")
                .with_description("Native plugin FFI call errors")
                .build(),
            panics_total: meter
                .u64_counter("plugin.panics")
                .with_description("Native plugin FFI call panics")
                .build(),
            timeouts_total: meter
                .u64_counter("plugin.timeouts")
                .with_description("Native plugin FFI call timeouts (caller-side)")
                .build(),
        }
    }

    /// Record a completed FFI call with pre-built labels.
    pub fn record(&self, labels: &[KeyValue; 2], duration_secs: f64, outcome: CallOutcome) {
        self.call_duration.record(duration_secs, labels);
        self.calls_total.add(1, labels);
        match outcome {
            CallOutcome::Success => {},
            CallOutcome::Error => {
                self.errors_total.add(1, labels);
            },
            CallOutcome::Panic => {
                self.panics_total.add(1, labels);
            },
        }
    }

    /// Record a timeout observed from the caller (async) side.
    ///
    /// Bumps `plugin.calls` (so the call attempt is counted) and
    /// `plugin.timeouts` (so timeouts are distinguishable from FFI
    /// errors).  Does **not** bump `plugin.errors`.
    ///
    /// If the worker eventually completes the wedged FFI call, the
    /// worker will record a second `plugin.calls` entry for the same
    /// logical call.  This is acceptable: `calls_total` counts
    /// observed completions + timeout attempts, not unique call IDs.
    pub fn record_timeout(&self, labels: &[KeyValue; 2]) {
        self.calls_total.add(1, labels);
        self.timeouts_total.add(1, labels);
    }

    /// Build a set of metric labels for a given plugin kind and operation.
    ///
    /// Allocates `kind` into a `String`; intended to be called once per
    /// instance (at construction time) and cached on [`InstanceState`].
    pub fn build_labels(kind: &str, op: &'static str) -> [KeyValue; 2] {
        [KeyValue::new("plugin.kind", kind.to_string()), KeyValue::new("op", op)]
    }
}

impl Default for PluginMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_outcome_is_copy() {
        let a = CallOutcome::Success;
        let b = a;
        // Both usable after the move — confirms Copy.
        let _ = (a, b);
    }

    #[test]
    fn call_outcome_debug_distinguishes_variants() {
        assert_eq!(format!("{:?}", CallOutcome::Success), "Success");
        assert_eq!(format!("{:?}", CallOutcome::Error), "Error");
        assert_eq!(format!("{:?}", CallOutcome::Panic), "Panic");
    }

    #[test]
    fn new_and_default_construct_without_panicking() {
        let _ = PluginMetrics::new();
        let _ = PluginMetrics::default();
    }

    #[test]
    fn record_handles_every_outcome() {
        let metrics = PluginMetrics::new();
        let labels = PluginMetrics::build_labels("test_kind", "process_packet");

        // Each outcome must traverse the corresponding match arm without panicking.
        // Against the default no-op meter provider these are observable only via
        // "did not panic"; under a real SdkMeterProvider they would also bump the
        // backing counters.
        metrics.record(&labels, 0.0, CallOutcome::Success);
        metrics.record(&labels, 0.025, CallOutcome::Error);
        metrics.record(&labels, 1.5, CallOutcome::Panic);
    }

    #[test]
    fn record_timeout_does_not_panic() {
        let metrics = PluginMetrics::new();
        let labels = PluginMetrics::build_labels("any", "flush");
        metrics.record_timeout(&labels);
    }

    #[test]
    fn build_labels_emits_kind_and_op_in_order() {
        let labels = PluginMetrics::build_labels("whisper", "process_packet");
        assert_eq!(labels[0].key.as_str(), "plugin.kind");
        assert_eq!(labels[0].value.as_str(), "whisper");
        assert_eq!(labels[1].key.as_str(), "op");
        assert_eq!(labels[1].value.as_str(), "process_packet");
    }

    #[test]
    fn build_labels_clones_kind_to_owned_string() {
        // `kind` is taken by reference but stored as String — confirm the labels
        // remain valid after the source goes out of scope.
        let labels = {
            let owned_kind = String::from("ephemeral_plugin");
            PluginMetrics::build_labels(&owned_kind, "tick")
        };
        assert_eq!(labels[0].value.as_str(), "ephemeral_plugin");
        assert_eq!(labels[1].value.as_str(), "tick");
    }

    #[test]
    fn build_labels_handles_empty_kind() {
        let labels = PluginMetrics::build_labels("", "process_packet");
        assert_eq!(labels[0].value.as_str(), "");
        assert_eq!(labels[1].value.as_str(), "process_packet");
    }

    #[test]
    fn record_accepts_extreme_durations() {
        let metrics = PluginMetrics::new();
        let labels = PluginMetrics::build_labels("kind", "op");
        // Inputs span far below the histogram floor (10μs), the floor itself,
        // and well above the ceiling (60s) defined in
        // crates/core/src/metrics.rs. record() must accept any positive
        // finite duration without panicking regardless of where it falls
        // relative to those bounds.
        metrics.record(&labels, f64::MIN_POSITIVE, CallOutcome::Success);
        metrics.record(&labels, 1e-6, CallOutcome::Success);
        metrics.record(&labels, 120.0, CallOutcome::Success);
    }
}
