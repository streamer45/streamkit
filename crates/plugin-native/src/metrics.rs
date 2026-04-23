// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! OpenTelemetry metrics for native plugin FFI calls.
//!
//! Instruments are built against the OTel global meter on first access.
//! The OTel meter provider must be initialized before the first plugin load.

use opentelemetry::metrics::{Counter, Histogram};
use opentelemetry::KeyValue;

/// Per-plugin metrics for FFI call instrumentation.
///
/// A single global instance is shared across all plugin instances;
/// individual calls are distinguished by `plugin.kind` and `op` labels.
pub struct PluginMetrics {
    pub call_duration: Histogram<f64>,
    pub calls_total: Counter<u64>,
    pub errors_total: Counter<u64>,
    pub panics_total: Counter<u64>,
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

    pub fn record_call(&self, kind: &str, op: &'static str, duration_secs: f64, success: bool) {
        let labels = [KeyValue::new("plugin.kind", kind.to_string()), KeyValue::new("op", op)];
        self.call_duration.record(duration_secs, &labels);
        self.calls_total.add(1, &labels);
        if !success {
            self.errors_total.add(1, &labels);
        }
    }

    pub fn record_panic(&self, kind: &str, op: &'static str) {
        let labels = [KeyValue::new("plugin.kind", kind.to_string()), KeyValue::new("op", op)];
        self.panics_total.add(1, &labels);
    }
}

impl Default for PluginMetrics {
    fn default() -> Self {
        Self::new()
    }
}
