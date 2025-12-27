// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! CPU profiling support via pprof-rs

#[cfg(feature = "profiling")]
use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
#[cfg(feature = "profiling")]
use pprof::{protos::Message, ProfilerGuardBuilder};
#[cfg(feature = "profiling")]
use serde::Deserialize;
#[cfg(feature = "profiling")]
use std::time::Duration;
#[cfg(feature = "profiling")]
use tokio::time::sleep;

#[cfg(feature = "profiling")]
#[derive(Deserialize)]
pub struct ProfileParams {
    #[serde(default = "default_duration")]
    duration_secs: u64,
    #[serde(default)]
    format: ProfileFormat,
    #[serde(default = "default_frequency")]
    frequency: i32,
}

#[cfg(feature = "profiling")]
const fn default_duration() -> u64 {
    30
}

#[cfg(feature = "profiling")]
const fn default_frequency() -> i32 {
    99 // Use 99 Hz (not 100) to avoid lock-step with other timers
}

#[cfg(feature = "profiling")]
#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "lowercase")]
enum ProfileFormat {
    #[default]
    Flamegraph,
    Protobuf,
}

/// CPU profiling endpoint that captures a profile for a specified duration
/// and returns either a flamegraph (SVG) or protobuf format
///
/// # Errors
///
/// Returns an error if:
/// - Duration is 0 or exceeds 300 seconds
/// - Frequency is outside the 1-10000 Hz range
/// - Profiling fails to start or stop
/// - Report generation fails
#[cfg(feature = "profiling")]
#[allow(clippy::cognitive_complexity)]
pub async fn profile_cpu(Query(params): Query<ProfileParams>) -> Result<Response, StatusCode> {
    // Validate parameters
    if params.duration_secs == 0 || params.duration_secs > 300 {
        tracing::warn!(duration = params.duration_secs, "Invalid profiling duration requested");
        return Err(StatusCode::BAD_REQUEST);
    }

    if params.frequency < 1 || params.frequency > 10000 {
        tracing::warn!(frequency = params.frequency, "Invalid profiling frequency requested");
        return Err(StatusCode::BAD_REQUEST);
    }

    tracing::info!(
        duration_secs = params.duration_secs,
        frequency = params.frequency,
        format = ?params.format,
        "Starting CPU profiling"
    );

    let guard = ProfilerGuardBuilder::default()
        .frequency(params.frequency)
        // Blocklist system libraries and the profiler's own stack trace collection
        // to avoid the profiler profiling itself (which causes 100% overhead)
        .blocklist(&["libc", "libgcc", "pthread", "vdso", "libunwind", "backtrace"])
        .build()
        .map_err(|e| {
            tracing::error!(error = %e, "Failed to build profiler");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Profile for the requested duration
    sleep(Duration::from_secs(params.duration_secs)).await;

    tracing::info!("CPU profiling completed, generating report");

    // Run blocking report generation on dedicated thread pool to avoid blocking the async runtime.
    // Symbol resolution and report building can take significant time with large profiles.
    let format = params.format;
    let result = tokio::task::spawn_blocking(
        move || -> Result<(Vec<u8>, &'static str, Option<&'static str>), String> {
            let report =
                guard.report().build().map_err(|e| format!("Failed to build report: {e}"))?;

            match format {
                ProfileFormat::Flamegraph => {
                    let mut body = Vec::new();
                    report
                        .flamegraph(&mut body)
                        .map_err(|e| format!("Failed to generate flamegraph: {e}"))?;
                    Ok((body, "image/svg+xml", None))
                },
                ProfileFormat::Protobuf => {
                    let profile =
                        report.pprof().map_err(|e| format!("Failed to generate pprof: {e}"))?;
                    let mut body = Vec::new();
                    profile
                        .encode(&mut body)
                        .map_err(|e| format!("Failed to encode protobuf: {e}"))?;
                    Ok((
                        body,
                        "application/octet-stream",
                        Some("attachment; filename=\"profile.pb\""),
                    ))
                },
            }
        },
    )
    .await
    .map_err(|e| {
        tracing::error!(error = %e, "Profiler task panicked");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    match result {
        Ok((body, content_type, content_disposition)) => {
            tracing::info!(size_bytes = body.len(), "Generated profile");
            if let Some(disposition) = content_disposition {
                Ok((
                    [
                        (header::CONTENT_TYPE, content_type),
                        (header::CONTENT_DISPOSITION, disposition),
                    ],
                    body,
                )
                    .into_response())
            } else {
                Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "Failed to generate profile");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        },
    }
}

/// Stub endpoint when profiling feature is disabled
#[cfg(not(feature = "profiling"))]
pub async fn profile_cpu() -> (axum::http::StatusCode, &'static str) {
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "Profiling feature not enabled. Rebuild with --features profiling",
    )
}

/// Heap profiling endpoint that captures the current heap state
///
/// # Errors
///
/// Returns an error if:
/// - PROF_CTL is not initialized (jemalloc profiling not enabled)
/// - Heap dump fails
/// - Report generation fails
#[cfg(feature = "profiling")]
pub async fn profile_heap() -> Result<Response, StatusCode> {
    tracing::info!("Starting heap profiling");

    // Get the global PROF_CTL and dump heap profile
    let pprof = {
        let mut prof_ctl = jemalloc_pprof::PROF_CTL
            .as_ref()
            .ok_or_else(|| {
                tracing::error!("PROF_CTL not initialized - jemalloc profiling not enabled");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
            .lock()
            .await;

        prof_ctl.dump_pprof().map_err(|e| {
            tracing::error!(error = %e, "Failed to dump heap profile");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    };

    tracing::info!(size_bytes = pprof.len(), "Generated heap profile");

    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CONTENT_DISPOSITION, "attachment; filename=\"heap.pb.gz\""),
        ],
        pprof,
    )
        .into_response())
}

/// Stub endpoint when profiling feature is disabled
#[cfg(not(feature = "profiling"))]
pub async fn profile_heap() -> (axum::http::StatusCode, &'static str) {
    (
        axum::http::StatusCode::NOT_IMPLEMENTED,
        "Profiling feature not enabled. Rebuild with --features profiling",
    )
}
