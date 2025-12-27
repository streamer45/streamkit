// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use clap::{Parser, Subcommand};
use schemars::schema_for;
use tracing::{error, info, warn};

use crate::config;

type LogInitFn =
    fn(
        &config::LogConfig,
        &config::TelemetryConfig,
    )
        -> Result<Option<tracing_appender::non_blocking::WorkerGuard>, Box<dyn std::error::Error>>;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Path to configuration file
    #[arg(short, long, default_value = "skit.toml")]
    pub config: String,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Starts the skit server
    Serve,
    /// Manage configuration
    #[command(subcommand)]
    Config(ConfigCommands),
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Generate a default config file and print it to stdout
    Default,
    /// Generate a JSON schema for the config and print it to stdout
    Schema,
}

/// Initialize telemetry (metrics) if enabled in configuration
/// Returns the meter provider that must be kept alive
#[allow(clippy::collection_is_never_read)] // Meter provider must be kept alive
fn init_telemetry_if_enabled(
    config: &config::Config,
) -> Option<opentelemetry_sdk::metrics::SdkMeterProvider> {
    if !config.telemetry.enable {
        return None;
    }

    match crate::telemetry::init_metrics(&config.telemetry) {
        Ok(provider) => {
            info!("OpenTelemetry metrics enabled");
            Some(provider)
        },
        Err(e) => {
            warn!(error = %e, "Failed to initialize OpenTelemetry metrics");
            None
        },
    }
}

/// Log server startup information
fn log_startup_info(config: &config::Config) {
    info!(
        address = %config.server.address,
        console_enable = config.log.console_enable,
        file_enable = config.log.file_enable,
        console_level = ?config.log.console_level,
        file_level = ?config.log.file_level,
        file_path = %config.log.file_path,
        "Starting skit server"
    );
}

/// Handle the "serve" command - start the server
/// Exits the process on error with status code 1
// Allow eprintln before logging is initialized (CLI output)
#[allow(clippy::disallowed_macros)]
async fn handle_serve_command(config_path: &str, init_logging: LogInitFn) {
    let config_result = match config::load(config_path) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        },
    };

    let _log_guard = match init_logging(&config_result.config.log, &config_result.config.telemetry)
    {
        Ok(guard) => guard,
        Err(e) => {
            eprintln!("Failed to initialize logging: {e}");
            std::process::exit(1);
        },
    };

    let _meter_provider = init_telemetry_if_enabled(&config_result.config);

    if let Some(missing_file) = &config_result.file_missing {
        warn!(config_path = %missing_file, "Config file not found, using defaults");
    }

    log_startup_info(&config_result.config);

    if config_result.config.telemetry.enable {
        crate::telemetry::start_system_metrics();
    }

    if let Err(e) = crate::server::start_server(&config_result.config).await {
        error!(error = %e, "Failed to start server");
        std::process::exit(1);
    }
}

/// Handle the "config default" command - print default config to stdout
// Allow println for CLI output to stdout (intentional)
#[allow(clippy::disallowed_macros)]
fn handle_config_default_command() {
    match config::generate_default() {
        Ok(toml_string) => {
            println!("# Default skit configuration file");
            println!("{toml_string}");
        },
        Err(e) => {
            eprintln!("Failed to generate default config: {e}");
            std::process::exit(1);
        },
    }
}

/// Handle the "config schema" command - print JSON schema to stdout
// Allow println for CLI output to stdout (intentional)
#[allow(clippy::disallowed_macros)]
fn handle_config_schema_command() {
    let schema = schema_for!(config::Config);
    match serde_json::to_string_pretty(&schema) {
        Ok(json) => {
            println!("{json}");
        },
        Err(e) => {
            eprintln!("Failed to generate config schema: {e}");
            std::process::exit(1);
        },
    }
}

/// Handle CLI commands
// Allow eprintln/println before logging is initialized (for CLI output)
#[allow(clippy::disallowed_macros)]
pub async fn handle_command(cli: &Cli, init_logging: LogInitFn) {
    match cli.command.as_ref().unwrap_or(&Commands::Serve) {
        Commands::Serve => {
            handle_serve_command(&cli.config, init_logging).await;
        },
        Commands::Config(ConfigCommands::Default) => {
            handle_config_default_command();
        },
        Commands::Config(ConfigCommands::Schema) => {
            handle_config_schema_command();
        },
    }
}
