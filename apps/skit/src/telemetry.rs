// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use opentelemetry::global;
use opentelemetry::trace::TracerProvider;
use opentelemetry_otlp::{Protocol, WithExportConfig, WithHttpConfig};
use opentelemetry_sdk::{
    metrics::{PeriodicReader, SdkMeterProvider},
    trace::{self as sdktrace, SdkTracerProvider},
    Resource,
};
use std::sync::Arc;
use std::time::Duration;
use sysinfo::System;
use tokio::sync::Mutex;
use tracing_opentelemetry::OpenTelemetryLayer;

use crate::config::TelemetryConfig;

/// Build OTLP metrics exporter with optional custom headers.
fn build_otlp_exporter(
    endpoint: &str,
    headers: &std::collections::HashMap<String, String>,
) -> Result<opentelemetry_otlp::MetricExporter, Box<dyn std::error::Error>> {
    let mut exporter_builder = opentelemetry_otlp::MetricExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(10));

    if !headers.is_empty() {
        tracing::info!("Adding {} custom headers to OTLP exporter", headers.len());
        exporter_builder = exporter_builder.with_headers(headers.clone());
    }

    exporter_builder.build().map_err(|e| {
        tracing::error!("Failed to build OTLP metrics exporter: {}", e);
        e.into()
    })
}

fn build_otlp_span_exporter(
    endpoint: &str,
    headers: &std::collections::HashMap<String, String>,
) -> Result<opentelemetry_otlp::SpanExporter, Box<dyn std::error::Error>> {
    let mut exporter_builder = opentelemetry_otlp::SpanExporter::builder()
        .with_http()
        .with_protocol(Protocol::HttpBinary)
        .with_endpoint(endpoint)
        .with_timeout(Duration::from_secs(10));

    if !headers.is_empty() {
        tracing::info!("Adding {} custom headers to OTLP trace exporter", headers.len());
        exporter_builder = exporter_builder.with_headers(headers.clone());
    }

    exporter_builder.build().map_err(|e| {
        tracing::error!("Failed to build OTLP trace exporter: {}", e);
        e.into()
    })
}

/// Initialize metrics provider with OTLP export.
fn init_metrics_with_otlp(
    builder: opentelemetry_sdk::metrics::MeterProviderBuilder,
    endpoint: &str,
    headers: &std::collections::HashMap<String, String>,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error>> {
    tracing::info!(endpoint = %endpoint, "Configuring OTLP metrics exporter");

    let exporter = build_otlp_exporter(endpoint, headers)?;
    tracing::info!("OTLP metrics exporter built successfully");

    let reader = PeriodicReader::builder(exporter).with_interval(Duration::from_secs(5)).build();

    let provider = builder.with_reader(reader).build();
    global::set_meter_provider(provider.clone());

    tracing::info!("OTLP exporter will send metrics to: {}", endpoint);
    tracing::info!("Metrics provider initialized and set globally");

    Ok(provider)
}

/// Initialize metrics provider without export (local collection only).
fn init_metrics_local_only(
    builder: opentelemetry_sdk::metrics::MeterProviderBuilder,
) -> SdkMeterProvider {
    tracing::info!("No OTLP endpoint configured, metrics will be collected but not exported");
    let provider = builder.build();
    global::set_meter_provider(provider.clone());
    provider
}

/// Initializes the OpenTelemetry metrics provider with optional OTLP export.
///
/// # Errors
///
/// Returns an error if:
/// - The OTLP metrics exporter fails to build (invalid endpoint, network issues)
/// - The metrics provider fails to initialize
///
pub fn init_metrics(
    config: &TelemetryConfig,
) -> Result<SdkMeterProvider, Box<dyn std::error::Error>> {
    tracing::info!(
        "Initializing metrics with config: enable={}, endpoint={:?}",
        config.enable,
        config.otlp_endpoint
    );

    let resource = Resource::builder_empty()
        .with_attributes([
            opentelemetry::KeyValue::new("service.name", "skit"),
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let builder = SdkMeterProvider::builder().with_resource(resource);

    if let Some(endpoint) = &config.otlp_endpoint {
        init_metrics_with_otlp(builder, endpoint, &config.otlp_headers)
    } else {
        Ok(init_metrics_local_only(builder))
    }
}

/// Starts system metrics collection - should be called after tokio runtime is available
pub fn start_system_metrics() {
    start_system_metrics_collection();
}

/// Starts system metrics collection using our own implementation
fn start_system_metrics_collection() {
    // Use new() instead of new_all() - only initialize what we need
    let system = Arc::new(Mutex::new(System::new()));

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        let meter = global::meter("skit_system");

        // Create gauges for system metrics
        let cpu_usage_gauge = meter
            .f64_gauge("system_cpu_utilization")
            .with_description("System-wide CPU utilization as a percentage")
            .with_unit("%")
            .build();

        let process_cpu_gauge = meter
            .f64_gauge("process_cpu_utilization")
            .with_description("Process CPU utilization normalized by number of CPUs (0-100%, follows OpenTelemetry semantic conventions)")
            .with_unit("%")
            .build();

        let memory_usage_gauge = meter
            .u64_gauge("system_memory_usage")
            .with_description("Used system memory in bytes")
            .with_unit("By")
            .build();

        let memory_total_gauge = meter
            .u64_gauge("system_memory_total")
            .with_description("Total system memory in bytes")
            .with_unit("By")
            .build();

        let process_memory_gauge = meter
            .u64_gauge("process_memory_usage")
            .with_description("Process memory usage in bytes")
            .with_unit("By")
            .build();

        tracing::info!("System metrics collection started");

        // Initial refresh to establish baseline for CPU measurements
        // sysinfo requires at least two measurements to calculate CPU usage
        {
            let system_clone = Arc::clone(&system);
            let _ = tokio::task::spawn_blocking(move || {
                let mut sys = system_clone.blocking_lock();
                sys.refresh_cpu_usage();
                if let Ok(current_pid) = sysinfo::get_current_pid() {
                    sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[current_pid]), true);
                }
            })
            .await;
        }

        loop {
            interval.tick().await;

            // Run blocking sysinfo operations on dedicated blocking thread pool
            // to avoid stealing tokio worker threads
            let system_clone = Arc::clone(&system);
            // Allow significant_drop_tightening: the Arc must be moved into the blocking task
            // and the mutex guard is properly dropped at the end of the closure
            #[allow(clippy::significant_drop_tightening)]
            let result = tokio::task::spawn_blocking(move || {
                let mut sys = system_clone.blocking_lock();

                // Refresh CPU and process stats - this will compute deltas from the previous refresh
                sys.refresh_cpu_usage();
                sys.refresh_memory();

                // Collect CPU usage (average across all cores)
                // Handle empty CPU list (shouldn't happen but be defensive)
                let cpu_usage: f64 = if sys.cpus().is_empty() {
                    0.0
                } else {
                    let cpu_count = sys.cpus().len();
                    let sum: f64 = sys.cpus().iter().map(|cpu| f64::from(cpu.cpu_usage())).sum();
                    // Cast is safe: CPU count will never exceed f64's precision in practice
                    #[allow(clippy::cast_precision_loss)]
                    {
                        sum / (cpu_count as f64)
                    }
                };

                // Collect system memory usage
                let used_memory = sys.used_memory();
                let total_memory = sys.total_memory();

                // Collect process-specific metrics (CPU and memory)
                // Cast is safe: CPU count will never exceed f64's precision in practice
                #[allow(clippy::cast_precision_loss)]
                let num_cpus = sys.cpus().len().max(1) as f64;
                let (process_cpu, process_memory) = sysinfo::get_current_pid()
                    .ok()
                    .and_then(|current_pid| {
                        // Refresh the current process with full stats (true parameter)
                        sys.refresh_processes(
                            sysinfo::ProcessesToUpdate::Some(&[current_pid]),
                            true,
                        );
                        sys.process(current_pid).map(|process| {
                            let process_memory = process.memory();
                            // Normalize CPU by number of CPUs to follow OpenTelemetry semantic conventions
                            // process.cpu.utilization = (CPU time delta) / (elapsed time * num_cpus)
                            // This makes it comparable to system.cpu.utilization (both 0-100%)
                            let process_cpu_raw = f64::from(process.cpu_usage());
                            let process_cpu = process_cpu_raw / num_cpus;
                            (Some(process_cpu), Some(process_memory))
                        })
                    })
                    .unwrap_or((None, None));

                (cpu_usage, used_memory, total_memory, process_cpu, process_memory)
            })
            .await;

            let (cpu_usage, used_memory, total_memory, process_cpu_usage, process_memory_usage) =
                result.unwrap_or_else(|e| {
                    tracing::warn!("Failed to collect system metrics: {}", e);
                    (0.0, 0, 0, None, None)
                });

            // Record metrics (non-blocking)
            cpu_usage_gauge.record(cpu_usage, &[]);
            memory_usage_gauge.record(used_memory, &[]);
            memory_total_gauge.record(total_memory, &[]);

            if let Some(process_cpu) = process_cpu_usage {
                process_cpu_gauge.record(process_cpu, &[]);
            }
            if let Some(process_memory) = process_memory_usage {
                process_memory_gauge.record(process_memory, &[]);
            }

            tracing::debug!(
                target: "skit::telemetry::system_metrics",
                system_cpu_usage = %cpu_usage,
                used_memory_mb = %(used_memory / 1024 / 1024),
                total_memory_mb = %(total_memory / 1024 / 1024),
                process_cpu_usage = ?process_cpu_usage,
                process_memory_mb = ?process_memory_usage.map(|m| m / 1024 / 1024),
                "Collected system metrics"
            );
        }
    });
}

/// Initializes an OpenTelemetry tracing layer that exports spans via OTLP.
///
/// # Errors
///
/// Returns an error if:
/// - `otlp_traces_endpoint` is missing
/// - The OTLP exporter cannot be constructed
/// - The tracer provider cannot be initialized
pub fn init_tracing_with_otlp<S>(
    config: &TelemetryConfig,
) -> Result<OpenTelemetryLayer<S, sdktrace::Tracer>, Box<dyn std::error::Error>>
where
    S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    let endpoint = config.otlp_traces_endpoint.as_ref().ok_or_else(|| {
        "Tracing is enabled but no `otlp_traces_endpoint` is configured".to_string()
    })?;

    tracing::info!(endpoint = %endpoint, "Configuring OTLP trace exporter");
    let exporter = build_otlp_span_exporter(endpoint, &config.otlp_headers)?;

    let resource = Resource::builder_empty()
        .with_attributes([
            opentelemetry::KeyValue::new("service.name", "skit"),
            opentelemetry::KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
        ])
        .build();

    let provider =
        SdkTracerProvider::builder().with_batch_exporter(exporter).with_resource(resource).build();

    let tracer = provider.tracer("skit");
    global::set_tracer_provider(provider);

    Ok(tracing_opentelemetry::layer().with_tracer(tracer))
}
