// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Ensure profiling and dhat-heap features are mutually exclusive
#[cfg(all(feature = "profiling", feature = "dhat-heap"))]
compile_error!(
    "Features 'profiling' and 'dhat-heap' are mutually exclusive. \
     Use 'profiling' for jemalloc heap snapshots, or 'dhat-heap' for allocation rate profiling."
);

// Enable jemalloc for heap profiling when profiling feature is enabled
#[cfg(feature = "profiling")]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// Configure jemalloc profiling at compile time
// This uses export_name to override the default malloc configuration
// It's required for jemalloc heap profiling and cannot be avoided
#[cfg(feature = "profiling")]
#[allow(unsafe_code)]
#[export_name = "malloc_conf"]
pub static MALLOC_CONF: &[u8] = b"prof:true,prof_active:true,lg_prof_sample:19\0";

// Enable DHAT for allocation rate profiling when dhat-heap feature is enabled
// DHAT tracks total allocations (not just live), making it ideal for finding hot allocation sites
#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

use clap::Parser;

mod assets;
mod cli;
mod config;
mod file_security;
mod logging;
#[cfg(feature = "moq")]
mod moq_gateway;
mod permissions;
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
    // Start DHAT profiler if enabled - must be created before any allocations we want to track
    // The profiler writes output to dhat-heap.json when dropped (on graceful shutdown)
    #[cfg(feature = "dhat-heap")]
    let _dhat_profiler = dhat::Profiler::new_heap();

    // Install default crypto provider for Rustls (required for HTTPS/TLS support)
    // This must be done before any TLS operations
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cli = cli::Cli::parse();
    cli::handle_command(&cli, |log_config, telemetry_config| {
        logging::init_logging(log_config, telemetry_config)
    })
    .await;

    // DHAT profiler is dropped here, writing dhat-heap.json
}
