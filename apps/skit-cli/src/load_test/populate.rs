// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use anyhow::{Context, Result};
use reqwest::multipart;
use std::path::Path;
use tokio::fs;
use tracing::{info, warn};

use crate::load_test::config::LoadTestConfig;

pub async fn populate_environment(config: &LoadTestConfig) -> Result<()> {
    let client = reqwest::Client::new();

    // Load native plugins
    for plugin_path in &config.populate.plugins_native {
        if let Err(e) = load_native_plugin(&client, &config.server.url, plugin_path).await {
            warn!("Failed to load native plugin {}: {}", plugin_path, e);
        }
    }

    // Load WASM plugins
    for plugin_path in &config.populate.plugins_wasm {
        if let Err(e) = load_wasm_plugin(&client, &config.server.url, plugin_path).await {
            warn!("Failed to load WASM plugin {}: {}", plugin_path, e);
        }
    }

    Ok(())
}

async fn load_native_plugin(client: &reqwest::Client, server_url: &str, path: &str) -> Result<()> {
    let path = Path::new(path);

    if !path.exists() {
        return Err(anyhow::anyhow!("Plugin file not found: {}", path.display()));
    }

    info!("Loading native plugin: {:?}", path);

    let file_name =
        path.file_name().and_then(|n| n.to_str()).context("Invalid plugin filename")?.to_string();

    let file_bytes = fs::read(path)
        .await
        .with_context(|| format!("Failed to read plugin file: {}", path.display()))?;

    let part = multipart::Part::bytes(file_bytes).file_name(file_name);

    let form = multipart::Form::new().part("plugin", part);

    // Unified endpoint: server accepts both native and WASM plugin uploads at /api/v1/plugins.
    let url = format!("{server_url}/api/v1/plugins");
    let response =
        client.post(&url).multipart(form).send().await.context("Failed to upload native plugin")?;

    if response.status().is_success() {
        info!("Successfully loaded native plugin: {}", path.display());
        Ok(())
    } else {
        let status = response.status();
        // Using unwrap_or_default is acceptable here: error text is for logging only,
        // empty string is a reasonable fallback if response body can't be read
        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("Failed to load native plugin: {status} - {body}"))
    }
}

async fn load_wasm_plugin(client: &reqwest::Client, server_url: &str, path: &str) -> Result<()> {
    let path = Path::new(path);

    if !path.exists() {
        return Err(anyhow::anyhow!("Plugin file not found: {}", path.display()));
    }

    info!("Loading WASM plugin: {:?}", path);

    let file_name =
        path.file_name().and_then(|n| n.to_str()).context("Invalid plugin filename")?.to_string();

    let file_bytes = fs::read(path)
        .await
        .with_context(|| format!("Failed to read plugin file: {}", path.display()))?;

    let part = multipart::Part::bytes(file_bytes).file_name(file_name);

    let form = multipart::Form::new().part("plugin", part);

    let url = format!("{server_url}/api/v1/plugins");
    let response =
        client.post(&url).multipart(form).send().await.context("Failed to upload WASM plugin")?;

    if response.status().is_success() {
        info!("Successfully loaded WASM plugin: {}", path.display());
        Ok(())
    } else {
        let status = response.status();
        // Using unwrap_or_default is acceptable here: error text is for logging only,
        // empty string is a reasonable fallback if response body can't be read
        let body = response.text().await.unwrap_or_default();
        Err(anyhow::anyhow!("Failed to load WASM plugin: {status} - {body}"))
    }
}
