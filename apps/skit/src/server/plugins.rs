// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use anyhow::{Context, Error as AnyhowError};
use axum::{
    extract::{multipart::MultipartError, Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tracing::{error, info, warn};

use crate::marketplace_installer::InstallPluginRequest;
use crate::marketplace_security::{origin_key, MarketplaceUrlPolicy, OriginKey};
use crate::plugin_paths;
use crate::plugin_records::{
    active_dir as plugin_active_dir, namespaced_kind as active_namespaced_kind, ActivePluginRecord,
};
use crate::state::AppState;

pub(super) async fn list_plugins_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    let mut plugins = app_state.plugin_manager.lock().await.list_plugins();

    plugins.retain(|plugin| perms.is_plugin_allowed(&plugin.kind));

    Json(plugins)
}

pub(super) async fn upload_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, PluginHttpError> {
    // Global hard gate: do not allow runtime plugin uploads unless explicitly enabled.
    if !app_state.config.plugins.http_management.allow_http_management {
        return Err(PluginHttpError::Forbidden(
            "Plugin uploads are disabled by configuration. Set [plugins].allow_http_management = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    if !perms.load_plugins {
        return Err(PluginHttpError::Forbidden(
            "Permission denied: cannot load plugins".to_string(),
        ));
    }

    let mut plugin_file_name: Option<String> = None;
    let mut temp_file_path: Option<std::path::PathBuf> = None;
    let mut declared_kind: Option<String> = None;

    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or("").to_string();
        if name == "kind" {
            if declared_kind.is_some() {
                return Err(PluginHttpError::BadRequest(
                    "Multiple 'kind' fields provided".to_string(),
                ));
            }
            let value = field.text().await.map_err(|e| {
                PluginHttpError::BadRequest(format!("Failed to read 'kind' field: {e}"))
            })?;
            let value = value.trim().to_string();
            if value.is_empty() {
                return Err(PluginHttpError::BadRequest(
                    "Plugin kind must not be empty".to_string(),
                ));
            }
            declared_kind = Some(value);
            continue;
        }

        if name != "plugin" {
            continue;
        }

        if let Some(existing) = temp_file_path.as_ref() {
            let _ = tokio::fs::remove_file(existing).await;
            return Err(PluginHttpError::BadRequest(
                "Multiple 'plugin' fields provided".to_string(),
            ));
        }

        let mut field = field;
        let file_name =
            field.file_name().map(std::string::ToString::to_string).ok_or_else(|| {
                PluginHttpError::BadRequest(
                    "Uploaded plugin file must include a filename".to_string(),
                )
            })?;

        // Stream upload to a temp file to avoid buffering large artifacts in memory.
        let tmp_name = format!("streamkit-plugin-upload-{}", uuid::Uuid::new_v4());
        let tmp_path = std::env::temp_dir().join(tmp_name);
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
            .await
            .map_err(|e| PluginHttpError::BadRequest(format!("Failed to create temp file: {e}")))?;

        let mut total_bytes: usize = 0;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    total_bytes = total_bytes.saturating_add(chunk.len());
                    if total_bytes > app_state.config.server.max_body_size {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return Err(PluginHttpError::BadRequest(format!(
                            "Plugin upload exceeds configured max body size ({} bytes)",
                            app_state.config.server.max_body_size
                        )));
                    }
                    if let Err(e) = file.write_all(&chunk).await {
                        let _ = tokio::fs::remove_file(&tmp_path).await;
                        return Err(PluginHttpError::BadRequest(format!(
                            "Failed to write temp file: {e}"
                        )));
                    }
                },
                Ok(None) => break,
                Err(e) => {
                    let _ = tokio::fs::remove_file(&tmp_path).await;
                    return Err(PluginHttpError::BadRequest(format!(
                        "Failed to read plugin upload stream: {e}"
                    )));
                },
            }
        }

        // Ensure all data is flushed to disk before we try to load the plugin.
        // This is important because load_from_temp_file uses sync file operations.
        if let Err(e) = file.flush().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(PluginHttpError::BadRequest(format!("Failed to flush temp file: {e}")));
        }
        if let Err(e) = file.sync_all().await {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(PluginHttpError::BadRequest(format!("Failed to sync temp file: {e}")));
        }
        // Explicitly drop the file handle to ensure it's closed before we read it
        drop(file);

        plugin_file_name = Some(file_name);
        temp_file_path = Some(tmp_path);
    }

    let file_name = plugin_file_name
        .ok_or_else(|| PluginHttpError::BadRequest("Missing 'plugin' file field".to_string()))?;
    let tmp_path = temp_file_path
        .ok_or_else(|| PluginHttpError::BadRequest("Missing 'plugin' file field".to_string()))?;

    let extension = std::path::Path::new(&file_name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let placeholder_kind = match extension {
        "wasm" => Some("plugin::wasm::placeholder"),
        "so" | "dylib" | "dll" => Some("plugin::native::placeholder"),
        _ => None,
    };

    if let Some(kind) = declared_kind.as_ref() {
        if let Some(placeholder) = placeholder_kind {
            let expected_prefix = if placeholder.starts_with("plugin::wasm::") {
                "plugin::wasm::"
            } else {
                "plugin::native::"
            };
            if !kind.starts_with(expected_prefix) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(PluginHttpError::BadRequest(format!(
                    "Declared plugin kind '{kind}' does not match uploaded file type"
                )));
            }
        }
        if !perms.is_plugin_allowed(kind) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::Forbidden(format!(
                "Permission denied: plugin '{kind}' not allowed"
            )));
        }
    } else if let Some(placeholder) = placeholder_kind {
        if !perms.is_plugin_allowed(placeholder) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::BadRequest(
                "Plugin kind must be provided when allowed_plugins is restricted".to_string(),
            ));
        }
    }

    let summary = match tokio::task::spawn_blocking({
        let manager = Arc::clone(&app_state.plugin_manager);
        let file_name = file_name.clone();
        let tmp_path = tmp_path.clone();
        move || {
            let mut mgr = manager.blocking_lock();
            mgr.load_from_temp_file(&file_name, &tmp_path)
        }
    })
    .await
    {
        Ok(Ok(summary)) => summary,
        Ok(Err(err)) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::from(err));
        },
        Err(err) => {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(PluginHttpError::BadRequest(format!("Plugin load task failed: {err}")));
        },
    };

    if !perms.is_plugin_allowed(&summary.kind) {
        let _ = tokio::task::spawn_blocking({
            let manager = Arc::clone(&app_state.plugin_manager);
            let kind = summary.kind.clone();
            move || {
                let mut mgr = manager.blocking_lock();
                let _ = mgr.unload_plugin(&kind, true);
            }
        })
        .await;

        return Err(PluginHttpError::Forbidden(format!(
            "Permission denied: plugin '{}' not allowed",
            summary.kind
        )));
    }

    if let Some(kind) = declared_kind.as_ref() {
        if summary.kind != *kind {
            let _ = tokio::task::spawn_blocking({
                let manager = Arc::clone(&app_state.plugin_manager);
                let summary_kind = summary.kind.clone();
                move || {
                    let mut mgr = manager.blocking_lock();
                    let _ = mgr.unload_plugin(&summary_kind, true);
                }
            })
            .await;

            return Err(PluginHttpError::BadRequest(format!(
                "Uploaded plugin kind '{}' does not match declared kind '{}'",
                summary.kind, kind
            )));
        }
    }

    Ok((StatusCode::CREATED, Json(summary)))
}

#[derive(Debug, Serialize)]
struct InstallPluginResponse {
    job_id: String,
}

pub(super) async fn install_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<InstallPluginRequest>,
) -> Result<impl IntoResponse, PluginHttpError> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err(PluginHttpError::Forbidden(
            "Marketplace installs are disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err(PluginHttpError::Forbidden(
            "Permission denied: cannot install plugins".to_string(),
        ));
    }

    if !app_state.marketplace_jobs.is_registry_configured(&request.registry) {
        return Err(PluginHttpError::BadRequest(format!(
            "Registry '{registry}' is not configured",
            registry = request.registry
        )));
    }

    let job_id = app_state.marketplace_jobs.enqueue(request, perms).await;
    Ok((StatusCode::ACCEPTED, Json(InstallPluginResponse { job_id })))
}

#[derive(Debug, Serialize)]
struct MarketplaceRegistry {
    id: String,
    url: String,
}

pub(super) async fn list_marketplace_registries_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            "Marketplace browsing is disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot view marketplace".to_string(),
        ));
    }

    let registries = app_state.marketplace_jobs.registries();
    let payload: Vec<MarketplaceRegistry> =
        registries.into_iter().map(|url| MarketplaceRegistry { id: url.clone(), url }).collect();
    Ok(Json(payload))
}

#[derive(Debug, Deserialize)]
pub(super) struct MarketplacePluginsQuery {
    registry: String,
    q: Option<String>,
}

pub(super) async fn validate_marketplace_registry_url(
    config: &crate::config::PluginConfig,
    registry: &str,
) -> anyhow::Result<(MarketplaceUrlPolicy, reqwest::Url, OriginKey)> {
    let policy = MarketplaceUrlPolicy::from_config(config);
    let registry_url = policy.validate_url("registry index", registry, None).await?;
    let registry_origin = origin_key(&registry_url)?;
    Ok((policy, registry_url, registry_origin))
}

pub(super) async fn validate_marketplace_plugin_urls(
    policy: &MarketplaceUrlPolicy,
    registry_origin: &OriginKey,
    version: &crate::marketplace::RegistryPluginVersion,
) -> anyhow::Result<(reqwest::Url, reqwest::Url)> {
    let manifest_url =
        policy.validate_url("manifest url", &version.manifest_url, Some(registry_origin)).await?;
    let signature_url_value = version
        .signature_url
        .as_deref()
        .map_or_else(|| format!("{}.minisig", manifest_url.as_str()), str::to_string);
    let signature_url =
        policy.validate_url("signature url", &signature_url_value, Some(registry_origin)).await?;
    Ok((manifest_url, signature_url))
}

pub(super) async fn list_marketplace_plugins_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<MarketplacePluginsQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            "Marketplace browsing is disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot view marketplace".to_string(),
        ));
    }

    let registry = query.registry.trim();
    if registry.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Registry is required".to_string()));
    }
    if !app_state.marketplace_jobs.is_registry_configured(registry) {
        return Err((StatusCode::BAD_REQUEST, format!("Registry '{registry}' is not configured")));
    }

    let (policy, registry_url, registry_origin) =
        validate_marketplace_registry_url(&app_state.config.plugins, registry)
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, format!("Registry URL rejected: {err}")))?;

    let registry_client = app_state.marketplace_jobs.registry_client();
    let mut index = registry_client
        .fetch_index_with_policy(&registry_url, &policy, &registry_origin)
        .await
        .map_err(|err| {
            (StatusCode::BAD_GATEWAY, format!("Failed to fetch registry index: {err}"))
        })?;

    let filter = query.q.as_deref().map(str::trim).filter(|val| !val.is_empty());
    if let Some(filter) = filter {
        let filter = filter.to_lowercase();
        index.plugins.retain(|plugin| marketplace_plugin_matches(plugin, &filter));
    }

    Ok(Json(index))
}

pub(super) fn marketplace_plugin_matches(
    plugin: &crate::marketplace::RegistryPlugin,
    filter: &str,
) -> bool {
    if plugin.id.to_lowercase().contains(filter) {
        return true;
    }
    if let Some(name) = plugin.name.as_ref() {
        if name.to_lowercase().contains(filter) {
            return true;
        }
    }
    if let Some(description) = plugin.description.as_ref() {
        if description.to_lowercase().contains(filter) {
            return true;
        }
    }
    false
}

#[derive(Debug, Deserialize)]
pub(super) struct MarketplacePluginQuery {
    registry: String,
    version: Option<String>,
}

#[derive(Debug, Serialize)]
struct MarketplaceSignatureStatus {
    verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    key_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct MarketplacePluginDetails {
    registry: String,
    plugin: crate::marketplace::RegistryPlugin,
    version: crate::marketplace::RegistryPluginVersion,
    manifest: crate::marketplace::PluginManifest,
    signature: MarketplaceSignatureStatus,
    allow_native_marketplace: bool,
}

pub(super) async fn get_marketplace_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(plugin_id): Path<String>,
    Query(query): Query<MarketplacePluginQuery>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    if !app_state.config.plugins.marketplace.marketplace_enabled {
        return Err((
            StatusCode::FORBIDDEN,
            "Marketplace browsing is disabled by configuration. Set [plugins].marketplace_enabled = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((
            StatusCode::FORBIDDEN,
            "Permission denied: cannot view marketplace".to_string(),
        ));
    }

    let registry = query.registry.trim();
    if registry.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Registry is required".to_string()));
    }
    if !app_state.marketplace_jobs.is_registry_configured(registry) {
        return Err((StatusCode::BAD_REQUEST, format!("Registry '{registry}' is not configured")));
    }

    let plugin_id = plugin_id.trim();
    if plugin_id.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "Plugin id is required".to_string()));
    }
    if let Err(err) = plugin_paths::validate_path_component("plugin id", plugin_id) {
        return Err((StatusCode::BAD_REQUEST, err.to_string()));
    }

    let (policy, registry_url, registry_origin) =
        validate_marketplace_registry_url(&app_state.config.plugins, registry)
            .await
            .map_err(|err| (StatusCode::BAD_REQUEST, format!("Registry URL rejected: {err}")))?;

    let registry_client = app_state.marketplace_jobs.registry_client();
    let signature_verifier = app_state.marketplace_jobs.verifier();
    let index = registry_client
        .fetch_index_with_policy(&registry_url, &policy, &registry_origin)
        .await
        .map_err(|err| {
            (StatusCode::BAD_GATEWAY, format!("Failed to fetch registry index: {err}"))
        })?;

    let plugin =
        index.plugins.into_iter().find(|plugin| plugin.id == plugin_id).ok_or_else(|| {
            (StatusCode::NOT_FOUND, format!("Plugin '{plugin_id}' not found in registry"))
        })?;

    let requested_version = query.version.as_deref().map(str::trim).filter(|val| !val.is_empty());
    let version_entry = if let Some(requested_version) = requested_version {
        plugin.versions.iter().find(|version| version.version == requested_version)
    } else if let Some(latest) = plugin.latest.as_ref() {
        plugin
            .versions
            .iter()
            .find(|version| version.version == *latest)
            .or_else(|| plugin.versions.first())
    } else {
        plugin.versions.first()
    }
    .cloned()
    .ok_or_else(|| {
        (StatusCode::NOT_FOUND, format!("No versions available for plugin '{plugin_id}'"))
    })?;

    let (manifest_url, signature_url) =
        validate_marketplace_plugin_urls(&policy, &registry_origin, &version_entry).await.map_err(
            |err| (StatusCode::BAD_GATEWAY, format!("Registry returned invalid URLs: {err}")),
        )?;

    let manifest_entry = registry_client
        .fetch_manifest_raw_with_policy(&manifest_url, &policy, &registry_origin)
        .await
        .map_err(|err| {
            (StatusCode::BAD_GATEWAY, format!("Failed to fetch plugin manifest: {err}"))
        })?;

    let signature = match registry_client
        .fetch_text_with_policy("signature url", &signature_url, &policy, &registry_origin)
        .await
    {
        Ok(signature_text) => {
            match signature_verifier.verify(manifest_entry.bytes.as_ref(), &signature_text) {
                Ok(verified_signature) => MarketplaceSignatureStatus {
                    verified: true,
                    key_id: Some(verified_signature.key_id),
                    error: None,
                },
                Err(err) => MarketplaceSignatureStatus {
                    verified: false,
                    key_id: None,
                    error: Some(err.to_string()),
                },
            }
        },
        Err(err) => MarketplaceSignatureStatus {
            verified: false,
            key_id: None,
            error: Some(err.to_string()),
        },
    };

    Ok(Json(MarketplacePluginDetails {
        registry: registry.to_string(),
        plugin,
        version: version_entry,
        manifest: manifest_entry.manifest,
        signature,
        allow_native_marketplace: app_state.config.plugins.marketplace.allow_native_marketplace,
    }))
}

pub(super) async fn find_active_record_for_kind(
    plugin_dir: &std::path::Path,
    kind: &str,
) -> Option<(std::path::PathBuf, ActivePluginRecord)> {
    let active_dir = plugin_active_dir(plugin_dir);
    let mut entries = match tokio::fs::read_dir(&active_dir).await {
        Ok(entries) => entries,
        Err(err) => {
            if err.kind() != std::io::ErrorKind::NotFound {
                warn!(error = %err, dir = ?active_dir, "Failed to read active plugin records");
            }
            return None;
        },
    };

    loop {
        let entry = match entries.next_entry().await {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(err) => {
                warn!(error = %err, dir = ?active_dir, "Failed to iterate active plugin records");
                break;
            },
        };

        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = match tokio::fs::read(&path).await {
            Ok(bytes) => bytes,
            Err(err) => {
                warn!(error = %err, file = ?path, "Failed to read active plugin record");
                continue;
            },
        };
        let record: ActivePluginRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(err) => {
                warn!(error = %err, file = ?path, "Failed to parse active plugin record");
                continue;
            },
        };
        if active_namespaced_kind(&record) == kind {
            return Some((path, record));
        }
    }

    None
}

pub(super) async fn remove_active_record_and_bundle(
    plugin_dir: &std::path::Path,
    record_path: &std::path::Path,
    record: &ActivePluginRecord,
) -> Result<(), anyhow::Error> {
    tokio::fs::remove_file(record_path).await.with_context(|| {
        format!("Failed to remove active record {record_path}", record_path = record_path.display())
    })?;

    let base_real = plugin_paths::canonicalize_existing_dir(plugin_dir).await?;
    plugin_paths::validate_path_component("plugin id", &record.plugin_id)?;
    plugin_paths::validate_path_component("plugin version", &record.version)?;

    let bundles_root = plugin_dir.join("bundles").join(&record.plugin_id);
    let bundle_dir = bundles_root.join(&record.version);
    if tokio::fs::try_exists(&bundle_dir).await.unwrap_or(false) {
        let bundle_dir_real =
            plugin_paths::ensure_existing_dir_under(&base_real, &bundle_dir, "bundle").await?;
        tokio::fs::remove_dir_all(&bundle_dir_real).await.with_context(|| {
            format!(
                "Failed to remove bundle directory {bundle_dir_real}",
                bundle_dir_real = bundle_dir_real.display()
            )
        })?;
    }

    if tokio::fs::try_exists(&bundles_root).await.unwrap_or(false) {
        let bundles_root_real =
            plugin_paths::ensure_existing_dir_under(&base_real, &bundles_root, "bundles").await?;
        let mut entries = tokio::fs::read_dir(&bundles_root_real).await?;
        if entries.next_entry().await?.is_none() {
            let _ = tokio::fs::remove_dir(&bundles_root_real).await;
        }
    }

    let cache_root = plugin_dir.join("cache").join(&record.plugin_id);
    if tokio::fs::try_exists(&cache_root).await.unwrap_or(false) {
        let cache_root_real =
            plugin_paths::ensure_existing_dir_under(&base_real, &cache_root, "cache").await?;
        let _ = tokio::fs::remove_dir_all(&cache_root_real).await;
    }

    Ok(())
}

#[derive(Debug, Default, Deserialize)]
pub(super) struct DeletePluginQuery {
    #[serde(default)]
    keep_file: bool,
}

pub(super) async fn delete_plugin_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(kind): Path<String>,
    Query(query): Query<DeletePluginQuery>,
) -> Result<impl IntoResponse, PluginHttpError> {
    // Global hard gate: do not allow runtime plugin deletion unless explicitly enabled.
    if !app_state.config.plugins.http_management.allow_http_management {
        return Err(PluginHttpError::Forbidden(
            "Plugin deletion is disabled by configuration. Set [plugins].allow_http_management = true to enable."
                .to_string(),
        ));
    }

    let perms = crate::role_extractor::get_permissions(&headers, &app_state);

    if !perms.delete_plugins {
        warn!(
            plugin_kind = %kind,
            delete_plugins = perms.delete_plugins,
            "Blocked attempt to delete plugin: permission denied"
        );
        return Err(PluginHttpError::Forbidden(
            "Permission denied: cannot delete plugins".to_string(),
        ));
    }

    let plugin_dir = std::path::PathBuf::from(&app_state.config.plugins.directory);
    let active_record = find_active_record_for_kind(&plugin_dir, &kind).await;

    info!(plugin_kind = %kind, keep_file = query.keep_file, "Deleting plugin");
    let summary = match tokio::task::spawn_blocking({
        let manager = Arc::clone(&app_state.plugin_manager);
        let kind = kind.clone();
        let remove_file = !query.keep_file;
        move || {
            let mut mgr = manager.blocking_lock();
            mgr.unload_plugin(&kind, remove_file)
        }
    })
    .await
    {
        Ok(Ok(summary)) => summary,
        Ok(Err(err)) => return Err(PluginHttpError::from(err)),
        Err(err) => {
            return Err(PluginHttpError::BadRequest(format!("Plugin unload task failed: {err}")));
        },
    };

    // Unregister any asset types owned by this plugin so the CRUD endpoints
    // stop serving stale types.  User-uploaded files are intentionally left
    // in place — they will become available again if the plugin is re-installed.
    //
    // For marketplace plugins use the authoritative `plugin_id` from the
    // active record (matches `manifest.id` used during registration).
    // For non-marketplace plugins fall back to `original_kind` which, by
    // convention, equals the manifest `id`.
    let unregister_id =
        active_record.as_ref().map_or(&summary.original_kind, |(_, record)| &record.plugin_id);
    app_state.plugin_asset_registry.unregister_plugin(unregister_id).await;

    if let Some((record_path, record)) = active_record {
        if query.keep_file {
            info!(
                plugin_id = %record.plugin_id,
                version = %record.version,
                "Kept marketplace bundle and active record for unloaded plugin"
            );
        } else if let Err(err) =
            remove_active_record_and_bundle(&plugin_dir, &record_path, &record).await
        {
            return Err(PluginHttpError::BadRequest(format!(
                "Failed to uninstall marketplace plugin: {err}"
            )));
        }
    }

    Ok(Json(summary))
}

pub(super) async fn get_job_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((StatusCode::FORBIDDEN, "Permission denied: cannot view jobs".to_string()));
    }

    app_state
        .marketplace_jobs
        .get_job(&job_id)
        .await
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Job '{job_id}' not found")))
}

pub(super) async fn cancel_job_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let perms = crate::role_extractor::get_permissions(&headers, &app_state);
    if !perms.load_plugins {
        return Err((StatusCode::FORBIDDEN, "Permission denied: cannot cancel jobs".to_string()));
    }

    app_state
        .marketplace_jobs
        .cancel_job(&job_id)
        .await
        .map(Json)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Job '{job_id}' not found")))
}

#[derive(Debug)]
pub(super) enum PluginHttpError {
    BadRequest(String),
    Forbidden(String),
    Multipart(MultipartError),
    Manager(AnyhowError),
}

impl IntoResponse for PluginHttpError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
            Self::Forbidden(msg) => (StatusCode::FORBIDDEN, msg).into_response(),
            Self::Multipart(err) => {
                error!(error = %err, "Multipart error processing plugin request");
                (StatusCode::BAD_REQUEST, format!("Invalid multipart payload: {err}"))
                    .into_response()
            },
            Self::Manager(err) => {
                error!(error = %err, "Plugin manager error");
                (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()).into_response()
            },
        }
    }
}

impl From<MultipartError> for PluginHttpError {
    fn from(err: MultipartError) -> Self {
        Self::Multipart(err)
    }
}

impl From<AnyhowError> for PluginHttpError {
    fn from(err: AnyhowError) -> Self {
        Self::Manager(err)
    }
}

#[cfg(test)]
mod marketplace_url_tests {
    use super::{validate_marketplace_plugin_urls, validate_marketplace_registry_url};
    use anyhow::{bail, Result};

    #[tokio::test]
    async fn browsing_rejects_cross_origin_manifest() -> Result<()> {
        let mut config = crate::config::PluginConfig::default();
        config.marketplace.security.marketplace_require_registry_origin = true;
        let (policy, _registry_url, registry_origin) =
            validate_marketplace_registry_url(&config, "https://registry.example.com/index.json")
                .await?;

        let version = crate::marketplace::RegistryPluginVersion {
            version: "1.0.0".to_string(),
            manifest_url: "https://evil.example.com/manifest.json".to_string(),
            signature_url: Some("https://registry.example.com/manifest.minisig".to_string()),
            published_at: None,
        };

        match validate_marketplace_plugin_urls(&policy, &registry_origin, &version).await {
            Ok(_) => bail!("expected origin rejection"),
            Err(err) => assert!(err.to_string().contains("origin")),
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // Panics ARE the assertions
mod handler_tests {
    use super::*;
    use crate::config::Config;
    use crate::permissions::Permissions;
    use axum::body::{to_bytes, Body};
    use axum::http::{header::CONTENT_TYPE, Method, Request, StatusCode};
    use axum::routing::{delete, get, post};
    use axum::Router;
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tower::ServiceExt;

    const MULTIPART_BOUNDARY: &str = "------------------------test-boundary";

    // role_extractor consults SK_ROLE before default_role.  A developer
    // (or CI runner) with SK_ROLE=admin in the environment would silently
    // upgrade no_perm_state() callers to admin perms and break the
    // FORBIDDEN assertions.  Clear it once at module entry.
    static ENV_SETUP: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    fn ensure_clean_env() {
        ENV_SETUP.get_or_init(|| std::env::remove_var("SK_ROLE"));
    }

    /// Returns a `Permissions` value with every plugin-related capability
    /// explicitly denied: no upload, no delete, no install, empty
    /// allow-list.  Starts from `Permissions::viewer()`, so unrelated
    /// read-only flags (`list_nodes`, `list_samples`, `read_samples`,
    /// etc.) remain enabled — do not reuse this helper for tests that
    /// rely on those being false.
    fn denied_plugin_perms() -> Permissions {
        let mut p = Permissions::viewer();
        p.list_sessions = false;
        p.load_plugins = false;
        p.delete_plugins = false;
        p.allowed_plugins = vec![];
        p
    }

    fn make_state_with(config: Config) -> (Arc<AppState>, TempDir) {
        ensure_clean_env();
        let tmp = TempDir::new().unwrap();
        let mut cfg = config;
        cfg.plugins.directory = tmp.path().to_string_lossy().into_owned();
        let state = crate::server::create_app_state(cfg, None);
        (state, tmp)
    }

    fn admin_state() -> (Arc<AppState>, TempDir) {
        let mut cfg = Config::default();
        cfg.plugins.http_management.allow_http_management = true;
        cfg.plugins.marketplace.marketplace_enabled = true;
        make_state_with(cfg)
    }

    fn locked_down_state() -> (Arc<AppState>, TempDir) {
        let cfg = Config::default();
        make_state_with(cfg)
    }

    fn no_perm_state() -> (Arc<AppState>, TempDir) {
        let mut cfg = Config::default();
        cfg.plugins.http_management.allow_http_management = true;
        cfg.plugins.marketplace.marketplace_enabled = true;
        cfg.permissions.default_role = "no-perms".to_string();
        cfg.permissions.roles.insert("no-perms".to_string(), denied_plugin_perms());
        make_state_with(cfg)
    }

    fn build_plugin_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/api/v1/plugins", get(list_plugins_handler).post(upload_plugin_handler))
            .route("/api/v1/plugins/{kind}", delete(delete_plugin_handler))
            .route("/api/v1/plugins/install", post(install_plugin_handler))
            .route("/api/v1/marketplace/registries", get(list_marketplace_registries_handler))
            .route("/api/v1/marketplace/plugins", get(list_marketplace_plugins_handler))
            .route("/api/v1/marketplace/plugins/{plugin_id}", get(get_marketplace_plugin_handler))
            .route("/api/v1/jobs/{job_id}", get(get_job_handler))
            .route("/api/v1/jobs/{job_id}/cancel", post(cancel_job_handler))
            .with_state(state)
    }

    async fn read_body(resp: axum::response::Response) -> (StatusCode, String) {
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    fn multipart_content_type() -> String {
        format!("multipart/form-data; boundary={MULTIPART_BOUNDARY}")
    }

    #[allow(clippy::type_complexity)] // test-only helper; explicit tuple shape aids readability
    fn multipart_body(parts: &[(&str, Option<&str>, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        for (name, filename, content_type, body) in parts {
            out.extend_from_slice(format!("--{MULTIPART_BOUNDARY}\r\n").as_bytes());
            let disp = filename.as_ref().map_or_else(
                || format!("Content-Disposition: form-data; name=\"{name}\"\r\n"),
                |fname| {
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n"
                    )
                },
            );
            out.extend_from_slice(disp.as_bytes());
            if let Some(ct) = content_type {
                out.extend_from_slice(format!("Content-Type: {ct}\r\n").as_bytes());
            }
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(body);
            out.extend_from_slice(b"\r\n");
        }
        out.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());
        out
    }

    fn post_multipart(uri: &str, body: Vec<u8>) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(CONTENT_TYPE, multipart_content_type())
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn list_plugins_returns_empty_when_no_plugins_loaded() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(Request::builder().uri("/api/v1/plugins").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed, json!([]));
    }

    #[tokio::test]
    async fn list_plugins_returns_empty_when_manager_is_empty_and_allowlist_is_empty() {
        // With no plugins loaded the `.retain()` at `list_plugins_handler`
        // is a no-op — this test only proves the empty-manager path
        // produces `[]` and that an empty allow-list does not panic.
        // It does NOT exercise the `is_plugin_allowed` filtering
        // decision; a future test that loads plugins into the manager
        // is required to cover that branch.
        let mut cfg = Config::default();
        cfg.permissions.default_role = "empty".to_string();
        cfg.permissions.roles.insert("empty".to_string(), denied_plugin_perms());
        let (state, _tmp) = make_state_with(cfg);

        let router = build_plugin_router(state);
        let resp = router
            .oneshot(Request::builder().uri("/api/v1/plugins").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "[]");
    }

    #[tokio::test]
    async fn upload_rejected_when_http_management_disabled() {
        let (state, _tmp) = locked_down_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[(
            "plugin",
            Some("test.so"),
            Some("application/octet-stream"),
            b"\x7fELF",
        )]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("disabled by configuration"), "body: {body}");
        assert!(body.contains("allow_http_management"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejected_when_load_plugins_permission_missing() {
        let (state, _tmp) = no_perm_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[(
            "plugin",
            Some("test.so"),
            Some("application/octet-stream"),
            b"\x7fELF",
        )]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("cannot load plugins"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_two_kind_fields() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[
            ("kind", None, None, b"plugin::native::a"),
            ("kind", None, None, b"plugin::native::b"),
        ]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Multiple 'kind' fields provided"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_whitespace_only_kind() {
        // Pins that the trim() at upload_plugin_handler treats
        // whitespace-only values as empty.
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[("kind", None, None, b"   ")]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Plugin kind must not be empty"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_literal_empty_kind() {
        // Pins that a literal-empty kind value is rejected by the same
        // branch as the whitespace case.
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[("kind", None, None, b"")]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Plugin kind must not be empty"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_missing_plugin_field() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[("kind", None, None, b"plugin::native::foo")]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Missing 'plugin' file field"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_plugin_field_without_filename() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[("plugin", None, Some("application/octet-stream"), b"raw")]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("must include a filename"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_duplicate_plugin_field() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[
            ("plugin", Some("a.so"), Some("application/octet-stream"), b"\x7fELF"),
            ("plugin", Some("b.so"), Some("application/octet-stream"), b"\x7fELF"),
        ]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Multiple 'plugin' fields provided"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_declared_kind_mismatching_file_extension() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        // .so file but declared as wasm kind → mismatch
        let body = multipart_body(&[
            ("kind", None, None, b"plugin::wasm::foo"),
            ("plugin", Some("foo.so"), Some("application/octet-stream"), b"\x7fELF"),
        ]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("does not match uploaded file type"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_rejects_declared_kind_outside_allowed_plugins() {
        // Configure a role that can load plugins generally but restricts
        // allowed_plugins to a narrow prefix that excludes our declared kind.
        let mut cfg = Config::default();
        cfg.plugins.http_management.allow_http_management = true;
        let mut perms = Permissions::admin();
        perms.allowed_plugins = vec!["plugin::wasm::allowed".to_string()];
        cfg.permissions.default_role = "strict".to_string();
        cfg.permissions.roles.insert("strict".to_string(), perms);
        let (state, _tmp) = make_state_with(cfg);

        let router = build_plugin_router(state);
        let body = multipart_body(&[
            ("kind", None, None, b"plugin::wasm::denied"),
            ("plugin", Some("foo.wasm"), Some("application/wasm"), b"\0asm"),
        ]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("not allowed"), "body: {body}");
    }

    #[tokio::test]
    async fn upload_invalid_native_artifact_reports_manager_error() {
        // Bytes are clearly not a valid .so, but the validations leading
        // up to load_from_temp_file all pass.  The plugin manager fails
        // to load the artifact, which surfaces as a
        // `PluginHttpError::Manager` (HTTP 422 UNPROCESSABLE_ENTITY).
        //
        // Pinning UNPROCESSABLE_ENTITY exactly (rather than accepting
        // 400 || 422) ensures the manager path is what failed:
        // BAD_REQUEST only occurs when `spawn_blocking` itself panics,
        // i.e. the manager was never reached.  Allowing both statuses
        // would silently mask a future change that short-circuits the
        // request before the load call.
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = multipart_body(&[(
            "plugin",
            Some("not-really-a-plugin.so"),
            Some("application/octet-stream"),
            b"not an ELF",
        )]);
        let resp = router.oneshot(post_multipart("/api/v1/plugins", body)).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "expected manager-side load failure, got {status}: {body}"
        );
        // Body comes from anyhow::Error::to_string() on the manager
        // failure — confirm it is non-empty so a future change that
        // silently swallows the underlying error message is caught.
        assert!(!body.is_empty(), "manager error body should not be empty");
    }

    #[tokio::test]
    async fn install_rejected_when_marketplace_disabled() {
        let (state, _tmp) = locked_down_state();
        let router = build_plugin_router(state);
        let body = json!({
            "registry": "https://example.com/index.json",
            "plugin_id": "foo",
        });
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/plugins/install")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("Marketplace installs are disabled"), "body: {body}");
    }

    #[tokio::test]
    async fn install_rejected_when_load_plugins_permission_missing() {
        let (state, _tmp) = no_perm_state();
        let router = build_plugin_router(state);
        let body = json!({
            "registry": "https://example.com/index.json",
            "plugin_id": "foo",
        });
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/plugins/install")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("cannot install plugins"), "body: {body}");
    }

    #[tokio::test]
    async fn install_rejects_unconfigured_registry() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let body = json!({
            "registry": "https://not-configured.example.com/index.json",
            "plugin_id": "foo",
        });
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/plugins/install")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("is not configured"), "body: {body}");
    }

    #[tokio::test]
    async fn list_registries_rejected_when_marketplace_disabled() {
        let (state, _tmp) = locked_down_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/registries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("Marketplace browsing is disabled"), "body: {body}");
    }

    #[tokio::test]
    async fn list_registries_rejected_when_load_plugins_missing() {
        let (state, _tmp) = no_perm_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/registries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("cannot view marketplace"), "body: {body}");
    }

    #[tokio::test]
    async fn list_registries_returns_configured_entries() {
        let mut cfg = Config::default();
        cfg.plugins.marketplace.marketplace_enabled = true;
        cfg.plugins.registries = vec!["https://example.com/index.json".to_string()];
        let (state, _tmp) = make_state_with(cfg);

        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/registries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::OK);
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed[0]["id"], "https://example.com/index.json");
        assert_eq!(parsed[0]["url"], "https://example.com/index.json");
    }

    #[tokio::test]
    async fn list_marketplace_plugins_rejected_when_marketplace_disabled() {
        let (state, _tmp) = locked_down_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/plugins?registry=https%3A%2F%2Fexample.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_marketplace_plugins_requires_non_empty_registry() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/plugins?registry=%20")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Registry is required"), "body: {body}");
    }

    #[tokio::test]
    async fn list_marketplace_plugins_rejects_unconfigured_registry() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri(
                        "/api/v1/marketplace/plugins?registry=https%3A%2F%2Funconfigured.example.com",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("is not configured"), "body: {body}");
    }

    #[tokio::test]
    async fn get_marketplace_plugin_rejected_when_marketplace_disabled() {
        let (state, _tmp) = locked_down_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/plugins/foo?registry=https%3A%2F%2Fexample.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn get_marketplace_plugin_requires_registry() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/plugins/foo?registry=")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("Registry is required"), "body: {body}");
    }

    #[tokio::test]
    async fn get_marketplace_plugin_rejects_unconfigured_registry() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(
                Request::builder()
                    .uri("/api/v1/marketplace/plugins/foo?registry=https%3A%2F%2Fnope.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("is not configured"), "body: {body}");
    }

    #[tokio::test]
    async fn delete_rejected_when_http_management_disabled() {
        let (state, _tmp) = locked_down_state();
        let router = build_plugin_router(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/plugins/plugin::native::foo")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("disabled by configuration"), "body: {body}");
        assert!(body.contains("allow_http_management"), "body: {body}");
    }

    #[tokio::test]
    async fn delete_rejected_when_delete_plugins_permission_missing() {
        let (state, _tmp) = no_perm_state();
        let router = build_plugin_router(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/plugins/plugin::native::foo")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("cannot delete plugins"), "body: {body}");
    }

    #[tokio::test]
    async fn delete_unknown_plugin_surfaces_manager_error_as_422() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/plugins/plugin::native::ghost")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        // The manager returns an Err for unknown plugins; the handler maps
        // this via `PluginHttpError::Manager` → 422 UNPROCESSABLE_ENTITY.
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn get_job_rejected_when_load_plugins_missing() {
        let (state, _tmp) = no_perm_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(Request::builder().uri("/api/v1/jobs/anything").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("cannot view jobs"), "body: {body}");
    }

    #[tokio::test]
    async fn get_job_unknown_returns_404() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let resp = router
            .oneshot(Request::builder().uri("/api/v1/jobs/missing-id").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("missing-id"), "body: {body}");
    }

    #[tokio::test]
    async fn cancel_job_rejected_when_load_plugins_missing() {
        let (state, _tmp) = no_perm_state();
        let router = build_plugin_router(state);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/jobs/anything/cancel")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("cannot cancel jobs"), "body: {body}");
    }

    #[tokio::test]
    async fn cancel_job_unknown_returns_404() {
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/jobs/missing-id/cancel")
            .body(Body::empty())
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(body.contains("missing-id"), "body: {body}");
    }

    #[tokio::test]
    async fn marketplace_plugin_matches_searches_id_name_description() {
        let plugin = crate::marketplace::RegistryPlugin {
            id: "alpha-id".to_string(),
            name: Some("Beta Display".to_string()),
            description: Some("Gamma summary".to_string()),
            latest: None,
            versions: vec![],
        };

        assert!(marketplace_plugin_matches(&plugin, "alpha"));
        assert!(marketplace_plugin_matches(&plugin, "beta"));
        assert!(marketplace_plugin_matches(&plugin, "gamma"));
        assert!(marketplace_plugin_matches(&plugin, "summary"));
        assert!(!marketplace_plugin_matches(&plugin, "zzz"));
    }

    #[tokio::test]
    async fn marketplace_plugin_matches_handles_missing_optional_fields() {
        let plugin = crate::marketplace::RegistryPlugin {
            id: "lone-id".to_string(),
            name: None,
            description: None,
            latest: None,
            versions: vec![],
        };
        assert!(marketplace_plugin_matches(&plugin, "lone"));
        assert!(!marketplace_plugin_matches(&plugin, "absent"));
    }

    #[tokio::test]
    async fn plugin_http_error_bad_request_maps_to_400() {
        let resp = PluginHttpError::BadRequest("oops".into()).into_response();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, "oops");
    }

    #[tokio::test]
    async fn plugin_http_error_forbidden_maps_to_403() {
        let resp = PluginHttpError::Forbidden("nope".into()).into_response();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body, "nope");
    }

    #[tokio::test]
    async fn plugin_http_error_manager_maps_to_422() {
        let resp = PluginHttpError::Manager(anyhow::anyhow!("downstream boom")).into_response();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert!(body.contains("downstream boom"), "body: {body}");
    }

    #[tokio::test]
    async fn plugin_http_error_multipart_maps_to_400_via_handler() {
        // The Multipart variant is only constructable from an axum
        // MultipartError, which we cannot fabricate directly.  Drive
        // a real multipart parse failure through the handler by sending
        // a body whose declared boundary does not match the body bytes
        // at all — multer raises `IncompleteFieldData` / similar before
        // any field is yielded, which `From<MultipartError>` wraps into
        // `PluginHttpError::Multipart`.
        //
        // Asserting on the literal "Invalid multipart payload" prefix
        // (formatted by the `Multipart` arm of `IntoResponse`) pins the
        // From<MultipartError> conversion specifically; the OR-fallback
        // to "Missing 'plugin' file field" would silently let a future
        // removal of that conversion pass.
        let (state, _tmp) = admin_state();
        let router = build_plugin_router(state);
        // Body containing the boundary marker but a truncated part with
        // no terminating `--BOUNDARY--`, no `\r\n\r\n` separating
        // headers from body, and no Content-Disposition.  multer
        // rejects this with a parse error.
        let bad_body = format!(
            "--{MULTIPART_BOUNDARY}\r\nbroken header without colon\r\n\r\nsome bytes but no closing boundary"
        );
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/plugins")
            .header(CONTENT_TYPE, multipart_content_type())
            .body(Body::from(bad_body.into_bytes()))
            .unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let (status, body) = read_body(resp).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(
            body.starts_with("Invalid multipart payload"),
            "expected `From<MultipartError>` mapping, got: {body}"
        );
    }
}
