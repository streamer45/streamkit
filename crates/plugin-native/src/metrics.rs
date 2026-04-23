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
#[derive(Clone, Copy)]
pub enum CallOutcome {
    Success,
    Error,
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
                self.errors_total.add(1, labels);
                self.panics_total.add(1, labels);
            },
        }
    }

    /// Record a timeout observed from the caller (async) side.
    pub fn record_timeout(&self, labels: &[KeyValue; 2]) {
        self.errors_total.add(1, labels);
    }

    /// Build a set of metric labels for a given plugin kind and operation.
    pub fn build_labels(kind: &str, op: &'static str) -> [KeyValue; 2] {
        [KeyValue::new("plugin.kind", kind.to_string()), KeyValue::new("op", op)]
    }
}

impl Default for PluginMetrics {
    fn default() -> Self {
        Self::new()
    }
}
