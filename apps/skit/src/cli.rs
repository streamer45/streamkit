// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use clap::{Parser, Subcommand};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use schemars::schema_for;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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

    /// Server base URL for API calls (defaults to the configured bind address)
    ///
    /// Examples:
    /// - `http://127.0.0.1:4545`
    /// - `https://demo.streamkit.dev:4545/s/session_abc`
    #[arg(long, env = "SKIT_SERVER_URL")]
    pub server_url: Option<String>,

    /// API token to authenticate CLI API calls (Bearer token)
    ///
    /// If not set, StreamKit will try to read `${auth.state_dir}/admin.token` from the config.
    #[arg(long, env = "SKIT_TOKEN")]
    pub token: Option<String>,

    /// Path to a file containing an API token (Bearer token)
    #[arg(long, env = "SKIT_TOKEN_FILE")]
    pub token_file: Option<String>,

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
    /// Manage authentication
    #[command(subcommand)]
    Auth(AuthCommands),
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Generate a default config file and print it to stdout
    Default,
    /// Generate a JSON schema for the config and print it to stdout
    Schema,
}

#[derive(Subcommand, Debug)]
pub enum AuthCommands {
    /// Print the bootstrap admin token path
    ///
    /// The admin token is automatically generated when auth is first initialized.
    /// Use this command to find where the token is stored.
    PrintAdminToken {
        /// Print only the token (for scripting)
        #[arg(long)]
        raw: bool,
    },
    /// Mint tokens (API or MoQ) and store metadata
    ///
    /// This is equivalent to creating tokens via the Web UI, and uses the HTTP API.
    #[command(subcommand)]
    Mint(MintTokenCommands),
    /// Rotate the signing key and mint a new admin token
    ///
    /// This will:
    /// 1. Generate a new signing key
    /// 2. Keep the old key for validating existing tokens
    /// 3. Mint a new admin token signed with the new key
    /// 4. Write the new token to the state directory
    RotateKey,
}

#[derive(Subcommand, Debug)]
pub enum MintTokenCommands {
    /// Mint an API token (aud: skit-api)
    Api {
        /// Role name (must exist in [permissions].roles)
        #[arg(long)]
        role: String,
        /// Optional label for UI display
        #[arg(long)]
        label: Option<String>,
        /// TTL in seconds (defaults to auth.api_default_ttl_secs)
        #[arg(long)]
        ttl_secs: Option<u64>,
        /// Output as JSON (useful for scripting)
        #[arg(long)]
        json: bool,
    },
    /// Mint a MoQ token (aud: skit-moq)
    ///
    /// Notes:
    /// - `--subscribe ''` / `--publish ''` (empty string) means "allow all"
    /// - Omitting the flag entirely means "allow none"
    #[cfg(feature = "moq")]
    Moq {
        /// URL path prefix the token applies to (e.g. /session/<id> or /moq/session1)
        #[arg(long)]
        root: String,
        /// Allowed broadcast prefixes to subscribe to (repeatable)
        #[arg(long)]
        subscribe: Vec<String>,
        /// Allowed broadcast prefixes to publish to (repeatable)
        #[arg(long)]
        publish: Vec<String>,
        /// Optional label for UI display
        #[arg(long)]
        label: Option<String>,
        /// TTL in seconds (defaults to auth.moq_default_ttl_secs)
        #[arg(long)]
        ttl_secs: Option<u64>,
        /// Output as JSON (useful for scripting)
        #[arg(long)]
        json: bool,
    },
}

fn normalize_base_path_for_url(base_path: Option<&str>) -> String {
    let Some(base_path) = base_path else {
        return String::new();
    };

    let trimmed = base_path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return String::new();
    }

    let trimmed = trimmed.trim_end_matches('/');
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

fn hostport_for_client(addr: SocketAddr) -> String {
    // Binding to "0.0.0.0"/"[::]" means "all interfaces"; for a local client use localhost.
    if addr.ip().is_unspecified() {
        return format!("localhost:{}", addr.port());
    }

    if addr.is_ipv6() {
        format!("[{}]:{}", addr.ip(), addr.port())
    } else {
        format!("{}:{}", addr.ip(), addr.port())
    }
}

fn default_server_url(config: &config::Config) -> Result<String, String> {
    let addr: SocketAddr = config
        .server
        .address
        .parse()
        .map_err(|e| format!("Invalid server.address '{}': {e}", config.server.address))?;
    let scheme = if config.server.tls { "https" } else { "http" };
    let hostport = hostport_for_client(addr);
    let base_path = normalize_base_path_for_url(config.server.base_path.as_deref());
    Ok(format!("{scheme}://{hostport}{base_path}"))
}

fn read_token_file(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("Failed to read token file '{}': {e}", path.display()))
}

fn resolve_cli_token(cli: &Cli, config: &config::Config) -> Result<String, String> {
    if let Some(token) = cli.token.as_deref() {
        let token = token.trim();
        if token.is_empty() {
            return Err("Empty --token".to_string());
        }
        return Ok(token.to_string());
    }

    if let Some(path) = cli.token_file.as_deref() {
        let token = read_token_file(Path::new(path))?;
        if token.is_empty() {
            return Err(format!("Token file '{path}' is empty"));
        }
        return Ok(token);
    }

    let token_path = PathBuf::from(&config.auth.state_dir).join("admin.token");
    if token_path.exists() {
        let token = read_token_file(&token_path)?;
        if token.is_empty() {
            return Err(format!("Bootstrap token file '{}' is empty", token_path.display()));
        }
        return Ok(token);
    }

    Err("No token provided. Pass --token/--token-file (or set SKIT_TOKEN/SKIT_TOKEN_FILE), or run this command on the server host where `${auth.state_dir}/admin.token` is readable.".to_string())
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

/// Handle the "auth print-admin-token" command
// Allow println/eprintln for CLI output (intentional)
#[allow(clippy::disallowed_macros)]
fn handle_auth_print_admin_token(config_path: &str) {
    let config_result = match config::load(config_path) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        },
    };

    let state_dir = std::path::Path::new(&config_result.config.auth.state_dir);
    let token_path = state_dir.join("admin.token");

    if token_path.exists() {
        println!("Admin token location: {}", token_path.display());
        println!();
        // Try to read and print the token
        match std::fs::read_to_string(&token_path) {
            Ok(token) => {
                println!("Token: {}", token.trim());
            },
            Err(e) => {
                eprintln!("Warning: Could not read token file: {e}");
                eprintln!("The file exists but may have restricted permissions.");
            },
        }
    } else {
        eprintln!("Admin token not found at: {}", token_path.display());
        eprintln!();
        eprintln!("The admin token is created when auth is first initialized.");
        eprintln!("Start the server with auth enabled to generate it:");
        eprintln!("  - Bind to a non-loopback address (auth.mode=auto)");
        eprintln!("  - Or set auth.mode=enabled in your config");
        std::process::exit(1);
    }
}

/// Handle the "auth print-admin-token --raw" command
// Allow println/eprintln for CLI output (intentional)
#[allow(clippy::disallowed_macros)]
fn handle_auth_print_admin_token_raw(config_path: &str) {
    let config_result = match config::load(config_path) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        },
    };

    let state_dir = std::path::Path::new(&config_result.config.auth.state_dir);
    let token_path = state_dir.join("admin.token");

    if !token_path.exists() {
        eprintln!("Admin token not found at: {}", token_path.display());
        std::process::exit(1);
    }

    match std::fs::read_to_string(&token_path) {
        Ok(token) => println!("{}", token.trim()),
        Err(e) => {
            eprintln!("Failed to read token file: {e}");
            std::process::exit(1);
        },
    }
}

/// Handle the "auth rotate-key" command
// Allow println/eprintln for CLI output (intentional)
#[allow(clippy::disallowed_macros)]
async fn handle_auth_rotate_key(cli: &Cli) {
    let config_result = match config::load(&cli.config) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        },
    };

    // Resolve the pre-rotation token and server URL so we can notify the
    // running server after the on-disk key files have been updated.  The
    // old token is still valid against the server's in-memory JWKS, which
    // is exactly what we need to authenticate the reload-keys call.
    let pre_rotate_token = resolve_cli_token(cli, &config_result.config).ok();
    let server_url = cli
        .server_url
        .as_deref()
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .or_else(|| default_server_url(&config_result.config).ok());

    // Initialize auth state (this will load existing keys)
    let auth_state = match crate::auth::AuthState::new(&config_result.config.auth, true).await {
        Ok(state) => state,
        Err(e) => {
            eprintln!("Failed to initialize auth: {e}");
            eprintln!();
            eprintln!("Make sure auth has been initialized by starting the server first.");
            std::process::exit(1);
        },
    };

    // Rotate the key
    match auth_state.rotate_key().await {
        Ok(key_material) => {
            println!("Key rotated successfully!");
            println!("New key ID: {}", key_material.kid);
            println!();

            // Mint a new admin token with the new key.  This persists
            // the JTI to tokens.json via the metadata store but does NOT
            // overwrite admin.token yet.
            match auth_state
                .mint_api_token(
                    "admin",
                    Some("bootstrap-admin"),
                    config_result.config.auth.api_max_ttl_secs,
                    "cli-rotate-key",
                )
                .await
            {
                Ok((token, _meta)) => {
                    // Notify the running server to reload *after* minting
                    // (so the server picks up the new JTI from tokens.json)
                    // but *before* overwriting admin.token (so the old token
                    // remains on disk if the notify call fails — avoiding an
                    // admin lockout).
                    notify_server_reload_keys(server_url.as_deref(), pre_rotate_token.as_deref())
                        .await;

                    // Now safe to overwrite admin.token.
                    let state_dir = std::path::Path::new(&config_result.config.auth.state_dir);
                    let token_path = state_dir.join("admin.token");

                    match crate::auth::FileKeyProvider::write_secure(&token_path, &token).await {
                        Ok(()) => {
                            println!("New admin token written to: {}", token_path.display());
                            println!();
                            println!("Token: {token}");
                        },
                        Err(e) => {
                            eprintln!("Warning: Could not write token file: {e}");
                            eprintln!("New admin token: {token}");
                        },
                    }
                },
                Err(e) => {
                    eprintln!("Failed to mint new admin token: {e}");
                    std::process::exit(1);
                },
            }
        },
        Err(e) => {
            eprintln!("Failed to rotate key: {e}");
            std::process::exit(1);
        },
    }
}

/// Best-effort POST to the running server's reload-keys endpoint.
///
/// Uses the pre-rotation admin token (which the server still recognises)
/// to tell it to re-read the on-disk JWKS.  Failures are printed as
/// warnings — the rotation itself has already succeeded on disk.
// Allow eprintln/println for CLI output (intentional)
#[allow(clippy::disallowed_macros)]
async fn notify_server_reload_keys(server_url: Option<&str>, token: Option<&str>) {
    let (Some(server_url), Some(token)) = (server_url, token) else {
        eprintln!();
        eprintln!(
            "Warning: Could not notify the running server (no server URL or token available)."
        );
        eprintln!("If the server is running, restart it or call POST /api/v1/auth/reload-keys.");
        return;
    };

    let url = format!("{server_url}/api/v1/auth/reload-keys");

    let Ok(auth_value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        eprintln!("Warning: pre-rotation token contains invalid header characters; skipping server notification.");
        return;
    };

    let mut headers = HeaderMap::new();
    headers.insert(AUTHORIZATION, auth_value);

    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .read_timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Warning: Could not build HTTP client ({e}); skipping server notification.");
            return;
        },
    };

    match client.post(&url).headers(headers).send().await {
        Ok(resp) if resp.status().is_success() => {
            println!();
            println!("Running server notified — keys reloaded.");
        },
        Ok(resp) => {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            // Truncate to avoid leaking sensitive data a proxy might echo.
            // Use chars() to stay on a char boundary (truncate panics mid-codepoint).
            let text: String = text.chars().take(256).collect();
            eprintln!();
            eprintln!("Warning: Server returned {status} for reload-keys: {text}");
            eprintln!("You may need to restart the server for the new key to take effect.");
        },
        Err(e) if e.is_connect() || e.is_timeout() => {
            // Server not reachable — likely not running. This is fine.
            println!();
            println!("Server not reachable ({e}); keys will be loaded on next startup.");
        },
        Err(e) => {
            eprintln!();
            eprintln!("Warning: Failed to notify server: {e}");
            eprintln!(
                "If the server is running, restart it or call POST /api/v1/auth/reload-keys."
            );
        },
    }
}

/// Handle the "auth mint api/moq" commands
// Allow println/eprintln for CLI output (intentional)
#[allow(clippy::disallowed_macros)]
async fn handle_auth_mint_token(cli: &Cli, cmd: &MintTokenCommands) {
    let config_result = match config::load(&cli.config) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Failed to load configuration: {e}");
            std::process::exit(1);
        },
    };

    let server_url = match cli.server_url.as_deref() {
        Some(url) => url.trim().trim_end_matches('/').to_string(),
        None => match default_server_url(&config_result.config) {
            Ok(url) => url,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            },
        },
    };

    let token = match resolve_cli_token(cli, &config_result.config) {
        Ok(token) => token,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        },
    };

    let mut headers = HeaderMap::new();
    let Ok(auth_value) = HeaderValue::from_str(&format!("Bearer {token}")) else {
        eprintln!("Invalid token (contains illegal header characters)");
        std::process::exit(1);
    };
    headers.insert(AUTHORIZATION, auth_value);

    let client = reqwest::Client::new();

    match cmd {
        MintTokenCommands::Api { role, label, ttl_secs, json } => {
            let body = crate::auth::handlers::CreateApiTokenRequest {
                role: role.clone(),
                label: label.clone().and_then(|s| {
                    let t = s.trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }),
                ttl_secs: *ttl_secs,
            };

            let url = format!("{server_url}/api/v1/auth/tokens");
            let resp = match client.post(&url).headers(headers.clone()).json(&body).send().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Failed to reach server at '{server_url}': {e}");
                    std::process::exit(1);
                },
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                eprintln!("Token mint failed ({status}): {text}");
                std::process::exit(1);
            }

            let out: crate::auth::handlers::CreateTokenResponse = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Failed to parse response JSON: {e}");
                    std::process::exit(1);
                },
            };

            if *json {
                match serde_json::to_string_pretty(&out) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("Failed to serialize JSON: {e}");
                        std::process::exit(1);
                    },
                }
            } else {
                println!("Token: {}", out.token);
                println!("jti: {}", out.jti);
                println!("exp: {}", out.exp);
            }
        },
        #[cfg(feature = "moq")]
        MintTokenCommands::Moq { root, subscribe, publish, label, ttl_secs, json } => {
            let root_trimmed = root.trim();
            if root_trimmed.is_empty() {
                eprintln!("Missing --root (use '/' to allow any path)");
                std::process::exit(1);
            }

            let normalized_root = if root_trimmed.starts_with('/') {
                root_trimmed.to_string()
            } else {
                format!("/{root_trimmed}")
            };

            let body = crate::auth::handlers::CreateMoqTokenRequest {
                root: normalized_root,
                subscribe: subscribe.clone(),
                publish: publish.clone(),
                label: label.clone().and_then(|s| {
                    let t = s.trim().to_string();
                    if t.is_empty() {
                        None
                    } else {
                        Some(t)
                    }
                }),
                ttl_secs: *ttl_secs,
            };

            let url = format!("{server_url}/api/v1/auth/moq-tokens");
            let resp = match client.post(&url).headers(headers.clone()).json(&body).send().await {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("Failed to reach server at '{server_url}': {e}");
                    std::process::exit(1);
                },
            };

            if !resp.status().is_success() {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                eprintln!("MoQ token mint failed ({status}): {text}");
                std::process::exit(1);
            }

            let out: crate::auth::handlers::CreateTokenResponse = match resp.json().await {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("Failed to parse response JSON: {e}");
                    std::process::exit(1);
                },
            };

            if *json {
                match serde_json::to_string_pretty(&out) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("Failed to serialize JSON: {e}");
                        std::process::exit(1);
                    },
                }
            } else {
                println!("Token: {}", out.token);
                println!("jti: {}", out.jti);
                println!("exp: {}", out.exp);
                if let Some(url) = out.url_template.as_deref() {
                    println!("url: {url}");
                }
            }
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
        Commands::Auth(AuthCommands::PrintAdminToken { raw }) => {
            if *raw {
                handle_auth_print_admin_token_raw(&cli.config);
            } else {
                handle_auth_print_admin_token(&cli.config);
            }
        },
        Commands::Auth(AuthCommands::Mint(cmd)) => {
            handle_auth_mint_token(cli, cmd).await;
        },
        Commands::Auth(AuthCommands::RotateKey) => {
            handle_auth_rotate_key(cli).await;
        },
    }
}
