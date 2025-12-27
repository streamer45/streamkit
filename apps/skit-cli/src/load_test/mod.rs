// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

pub mod config;
mod metrics;
mod populate;
mod scenarios;
mod workers;

use anyhow::{Context, Result};
use tokio::signal;
use tracing::{info, warn};

use crate::load_test::config::{LoadTestConfig, Scenario};
use crate::load_test::metrics::MetricsCollector;
use crate::load_test::populate::populate_environment;
use crate::load_test::scenarios::{run_dynamic_scenario, run_mixed_scenario, run_oneshot_scenario};

/// Runs a load test based on the provided configuration
///
/// # Errors
///
/// Returns an error if:
/// - Configuration file cannot be loaded or parsed
/// - Environment population (plugin loading) fails
/// - Scenario execution fails
#[allow(clippy::cognitive_complexity)]
pub async fn run_load_test(
    config_path: &str,
    server_override: Option<String>,
    duration_override: Option<u64>,
    cleanup: bool,
) -> Result<()> {
    // Load and parse config
    let mut config = LoadTestConfig::from_file(config_path)
        .with_context(|| format!("Failed to load config from {config_path}"))?;

    // Apply CLI overrides
    if let Some(server) = server_override {
        config.server.url = server;
    }
    if let Some(duration) = duration_override {
        config.test.duration_secs = duration;
    }

    config.validate()?;

    info!("Load test configuration loaded from: {}", config_path);
    info!("Server: {}", config.server.url);
    info!("Scenario: {:?}", config.test.scenario);
    info!("Duration: {}s", config.test.duration_secs);

    // Set up graceful shutdown handler
    let shutdown_token = tokio_util::sync::CancellationToken::new();
    let shutdown_handle = shutdown_token.clone();
    let ctrl_c_handle = tokio::spawn(async move {
        tokio::select! {
            _ = signal::ctrl_c() => {
                warn!("Received Ctrl+C, shutting down gracefully...");
                shutdown_handle.cancel();
            }
            () = shutdown_handle.cancelled() => {}
        }
    });

    // Populate environment (load plugins, etc.)
    if config.populate.load_plugins {
        info!("Pre-loading plugins...");
        populate_environment(&config).await?;
    }

    // Initialize metrics collector
    let metrics = MetricsCollector::new();

    // Run the appropriate scenario
    let result = match config.test.scenario {
        Scenario::OneShot => run_oneshot_scenario(&config, metrics, shutdown_token, cleanup).await,
        Scenario::Dynamic => run_dynamic_scenario(&config, metrics, shutdown_token, cleanup).await,
        Scenario::Mixed => run_mixed_scenario(&config, metrics, shutdown_token, cleanup).await,
    };

    match result {
        Ok(final_metrics) => {
            println!("\n{}", "=".repeat(60));
            println!("Load Test Complete");
            println!("{}", "=".repeat(60));
            match config.output.format {
                crate::load_test::config::OutputFormat::Text => {
                    final_metrics.print_summary();
                },
                crate::load_test::config::OutputFormat::Json => {
                    let report = final_metrics.as_report();
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report)
                            .context("Failed to serialize metrics as JSON")?
                    );
                },
                crate::load_test::config::OutputFormat::Csv => {
                    println!("{}", final_metrics.as_csv());
                },
            }

            ctrl_c_handle.abort();
            Ok(())
        },
        Err(e) => {
            warn!("Load test failed: {}", e);
            ctrl_c_handle.abort();
            Err(e)
        },
    }
}
