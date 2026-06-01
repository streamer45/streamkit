// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

#[cfg(all(feature = "profiling", feature = "dhat-heap"))]
compile_error!(
    "Features 'profiling' and 'dhat-heap' are mutually exclusive. \
     Use 'profiling' for jemalloc heap snapshots, or 'dhat-heap' for allocation rate profiling."
);

#[cfg(feature = "profiling")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[cfg(feature = "profiling")]
#[allow(unsafe_code)]
#[export_name = "malloc_conf"]
pub static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use clap::Parser;

mod assets;
mod auth;
mod cli;
mod config;
mod file_security;
mod log_viewer;
mod logging;
mod marketplace;
mod marketplace_installer;
mod marketplace_security;
#[cfg(feature = "mcp")]
mod mcp;
mod metrics_labels;
#[cfg(feature = "moq")]
mod moq_gateway;
mod mse_gateway;
mod permissions;
mod plugin_assets;
mod plugin_paths;
mod plugin_records;
mod plugins;
mod profiling;
mod role_extractor;
mod samples;
mod server;
mod session;
mod state;
mod telemetry;
mod websocket;
mod websocket_handlers;

#[tokio::main]
async fn main() {
    #[cfg(feature = "dhat-heap")]
    let _dhat_profiler = dhat::Profiler::new_heap();

    // Must be called before any TLS operations.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = cli::Cli::parse();
    cli::handle_command(&cli, |log_config, telemetry_config| {
        logging::init_logging(log_config, telemetry_config)
    })
    .await;
}
