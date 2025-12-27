// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use tracing_subscriber::{
    layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer, Registry,
};

use crate::config::{self, LogFormat};
use crate::telemetry;

type DynLayer = Box<dyn Layer<Registry> + Send + Sync + 'static>;

const fn more_verbose_level(a: tracing::Level, b: tracing::Level) -> tracing::Level {
    use tracing::Level;

    match (a, b) {
        (Level::TRACE, _) | (_, Level::TRACE) => Level::TRACE,
        (Level::DEBUG, _) | (_, Level::DEBUG) => Level::DEBUG,
        (Level::INFO, _) | (_, Level::INFO) => Level::INFO,
        (Level::WARN, _) | (_, Level::WARN) => Level::WARN,
        (Level::ERROR, Level::ERROR) => Level::ERROR,
    }
}

fn env_filter_or_level(default_level: tracing::Level) -> EnvFilter {
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level.as_str()))
}

fn make_console_layer(console_level: tracing::Level) -> DynLayer {
    tracing_subscriber::fmt::layer().with_filter(env_filter_or_level(console_level)).boxed()
}

fn make_file_layer(
    non_blocking: tracing_appender::non_blocking::NonBlocking,
    file_level: tracing::Level,
    file_format: LogFormat,
) -> DynLayer {
    match file_format {
        LogFormat::Json => tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .json()
            .with_filter(env_filter_or_level(file_level))
            .boxed(),
        LogFormat::Text => tracing_subscriber::fmt::layer()
            .with_writer(non_blocking)
            .with_ansi(false)
            .with_filter(env_filter_or_level(file_level))
            .boxed(),
    }
}

fn telemetry_default_level_for_config(log_config: &config::LogConfig) -> tracing::Level {
    let console_level: tracing::Level = log_config.console_level.clone().into();
    let file_level: tracing::Level = log_config.file_level.clone().into();

    match (log_config.console_enable, log_config.file_enable) {
        (true, true) => more_verbose_level(console_level, file_level),
        (true, false) => console_level,
        (false, true) => file_level,
        (false, false) => tracing::Level::INFO,
    }
}

const fn should_enable_otel_tracing(telemetry_config: &config::TelemetryConfig) -> bool {
    telemetry_config.enable
        && telemetry_config.tracing_enable
        && telemetry_config.otlp_traces_endpoint.is_some()
}

/// Initialize logging based on configuration
///
/// Sets up tracing subscribers for console and/or file logging with optional OpenTelemetry support.
///
/// # Errors
///
/// Returns an error if:
/// - Invalid log level directives are provided
/// - File logging is enabled but the log directory cannot be created
/// - OpenTelemetry initialization fails
///
/// # Panics
///
/// Panics if log file path parsing fails (when `unwrap_or_else` defaults are needed).
/// This is extremely unlikely as the paths always have valid parent/filename components.
#[allow(clippy::too_many_lines)] // Complexity is from handling all config combinations
pub fn init_logging(
    log_config: &config::LogConfig,
    telemetry_config: &config::TelemetryConfig,
) -> Result<Option<tracing_appender::non_blocking::WorkerGuard>, Box<dyn std::error::Error>> {
    let mut guard = None;

    // Only enable OpenTelemetry tracing layer when an OTLP traces endpoint is configured.
    // The OpenTelemetry layer adds ~8% CPU overhead even when not exporting,
    // so we skip it entirely for local-only deployments.
    let enable_otel = should_enable_otel_tracing(telemetry_config);

    if telemetry_config.enable
        && telemetry_config.tracing_enable
        && telemetry_config.otlp_traces_endpoint.is_none()
    {
        tracing::warn!(
            "OpenTelemetry tracing is enabled but `otlp_traces_endpoint` is not set; tracing will be disabled"
        );
    }

    #[cfg(not(feature = "tokio-console"))]
    if telemetry_config.tokio_console {
        tracing::warn!(
            "tokio_console=true but this build does not include the `tokio-console` feature"
        );
    }

    // Helper to set up file appender
    let setup_file_appender = |log_config: &config::LogConfig| -> Result<
        (tracing_appender::non_blocking::NonBlocking, tracing_appender::non_blocking::WorkerGuard),
        Box<dyn std::error::Error>,
    > {
        let log_path = std::path::Path::new(&log_config.file_path);
        let log_dir = log_path.parent().unwrap_or_else(|| std::path::Path::new("."));
        let log_filename = log_path.file_name().unwrap_or_else(|| std::ffi::OsStr::new("skit.log"));

        if let Err(e) = std::fs::create_dir_all(log_dir) {
            return Err(
                format!("Failed to create log directory {}: {}", log_dir.display(), e).into()
            );
        }

        let file_appender = tracing_appender::rolling::never(log_dir, log_filename);
        Ok(tracing_appender::non_blocking(file_appender))
    };

    let mut layers: Vec<DynLayer> = Vec::new();

    #[cfg(feature = "tokio-console")]
    if telemetry_config.tokio_console {
        let tokio_console_layer =
            console_subscriber::ConsoleLayer::builder().with_default_env().spawn();
        layers.push(tokio_console_layer.boxed());
    }

    if log_config.file_enable {
        let (non_blocking, file_guard) = setup_file_appender(log_config)?;
        guard = Some(file_guard);
        let file_level: tracing::Level = log_config.file_level.clone().into();
        layers.push(make_file_layer(non_blocking, file_level, log_config.file_format));
    }

    if log_config.console_enable {
        let console_level: tracing::Level = log_config.console_level.clone().into();
        layers.push(make_console_layer(console_level));
    }

    if !log_config.console_enable && !log_config.file_enable {
        layers.push(make_console_layer(tracing::Level::INFO));
        tracing::warn!(
            "Both console and file logging are disabled, falling back to console logging"
        );
    }

    if enable_otel {
        let telemetry_default_level = telemetry_default_level_for_config(log_config);
        layers.push(
            telemetry::init_tracing_with_otlp(telemetry_config)?
                .with_filter(env_filter_or_level(telemetry_default_level))
                .boxed(),
        );
    }

    tracing_subscriber::registry().with(layers).init();

    #[cfg(feature = "tokio-console")]
    if telemetry_config.tokio_console {
        tracing::info!("tokio-console subscriber enabled (listening on port 6669)");
    }

    if enable_otel {
        tracing::info!("OpenTelemetry tracing layer enabled");
    }

    Ok(guard)
}
