// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Plugin Asset Registry — generic CRUD for plugin-declared asset types.
//!
//! Plugins declare asset types in their `plugin.yml` manifest.  When loaded,
//! those declarations are registered here.  The server exposes uniform REST
//! endpoints under `/api/v1/assets/plugin/{type_id}` and a discovery endpoint
//! at `GET /api/v1/asset-types`.

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::marketplace::{AssetContentType, PluginAssetSpec};
use crate::permissions::Permissions as RolePermissions;
use crate::role_extractor::get_permissions;
use crate::state::AppState;
use streamkit_api::{AssetTypeInfo, PluginAsset};

// Security limits
const MAX_FILENAME_LENGTH: usize = 255;

// ── Registered asset type ────────────────────────────────────────────────────

/// A fully resolved asset type registered by a loaded plugin.
#[derive(Debug, Clone)]
pub struct RegisteredAssetType {
    /// Short identifier (e.g. `slint`).
    pub type_id: String,
    /// Plugin that owns this type.
    pub plugin_id: String,
    /// Namespaced node kind (e.g. `plugin::native::slint`).
    pub node_kind: String,
    /// Human-readable label.
    pub label: String,
    /// Allowed file extensions.
    pub extensions: Vec<String>,
    /// Maximum upload size in bytes.
    pub max_size_bytes: usize,
    /// Text or binary content.
    pub content_type: AssetContentType,
    /// UI icon hint.
    ///
    /// `Option` internally because plugins may omit it; the discovery API
    /// response (`AssetTypeInfo.icon_hint`) always fills in a default of
    /// `"file"` so the frontend never sees `None`.
    pub icon_hint: Option<String>,
    /// Node parameter that references this asset.
    pub node_param: Option<String>,
    /// Directory containing system (bundled) assets.
    pub system_dir: PathBuf,
    /// Directory for user-uploaded assets.
    pub user_dir: PathBuf,
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// Thread-safe registry of plugin-declared asset types.
///
/// Stored in [`AppState`] and shared across handlers.  Uses an `RwLock` so
/// reads (listing, serving) don't block each other.
#[derive(Debug, Default, Clone)]
pub struct PluginAssetRegistry {
    inner: Arc<RwLock<HashMap<String, RegisteredAssetType>>>,
}

impl PluginAssetRegistry {
    pub fn new() -> Self {
        Self { inner: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Register asset types declared by a plugin.
    ///
    /// `plugin_id` and `node_kind` come from the plugin manifest.
    /// Returns the number of types successfully registered; invalid specs
    /// (bad `type_id` or `system_dir` with path-traversal components) are
    /// logged and skipped.
    pub async fn register(&self, plugin_id: &str, node_kind: &str, specs: &[PluginAssetSpec]) {
        let mut map = self.inner.write().await;
        for spec in specs {
            // Reject type_id values that aren't URL-safe identifiers.
            if spec.type_id.is_empty()
                || !spec.type_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                warn!(
                    type_id = %spec.type_id,
                    plugin_id = %plugin_id,
                    "Skipping plugin asset type: type_id must be [a-zA-Z0-9_-]+"
                );
                continue;
            }

            // Reject system_dir with path-traversal or absolute path components.
            if let Some(ref dir) = spec.system_dir {
                let path = std::path::Path::new(dir);
                let has_traversal = path.components().any(|c| {
                    matches!(
                        c,
                        std::path::Component::ParentDir
                            | std::path::Component::RootDir
                            | std::path::Component::Prefix(_)
                    )
                });
                if has_traversal {
                    warn!(
                        type_id = %spec.type_id,
                        plugin_id = %plugin_id,
                        system_dir = %dir,
                        "Skipping plugin asset type: system_dir must be a relative path without '..'"
                    );
                    continue;
                }
            }

            let system_dir = spec.system_dir.as_deref().map_or_else(
                || PathBuf::from(format!("samples/{}/system", spec.type_id)),
                PathBuf::from,
            );
            let user_dir = system_dir.parent().map_or_else(
                || PathBuf::from(format!("samples/{}/user", spec.type_id)),
                |p| p.join("user"),
            );

            let registered = RegisteredAssetType {
                type_id: spec.type_id.clone(),
                plugin_id: plugin_id.to_string(),
                node_kind: node_kind.to_string(),
                label: spec.label.clone(),
                extensions: spec.extensions.clone(),
                max_size_bytes: spec.max_size_bytes,
                content_type: spec.content_type.clone(),
                icon_hint: spec.icon_hint.clone(),
                node_param: spec.node_param.clone(),
                system_dir,
                user_dir,
            };

            info!(
                type_id = %spec.type_id,
                plugin_id = %plugin_id,
                extensions = ?spec.extensions,
                "Registered plugin asset type"
            );
            map.insert(spec.type_id.clone(), registered);
        }
    }

    /// Remove all asset types owned by a plugin.
    ///
    /// Not yet called — will be wired into the plugin unload path.
    #[allow(dead_code)]
    pub async fn unregister_plugin(&self, plugin_id: &str) {
        let mut map = self.inner.write().await;
        let before = map.len();
        map.retain(|_, v| v.plugin_id != plugin_id);
        let removed = before - map.len();
        drop(map);
        if removed > 0 {
            info!(plugin_id = %plugin_id, removed, "Unregistered plugin asset types");
        }
    }

    /// Look up a registered type by its `type_id`.
    pub async fn get(&self, type_id: &str) -> Option<RegisteredAssetType> {
        self.inner.read().await.get(type_id).cloned()
    }

    /// Return all registered plugin asset types.
    pub async fn all(&self) -> Vec<RegisteredAssetType> {
        self.inner.read().await.values().cloned().collect()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Sanitize filename by removing dangerous characters.
///
/// After sanitization, rejects results that resolve to `.` or `..` (directory
/// references) to avoid path confusion downstream.
fn sanitize_filename(filename: &str) -> String {
    let sanitized: String = filename
        .chars()
        .map(
            |c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' },
        )
        .collect();

    // Guard against empty input and directory-reference results.
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        return String::from("_invalid_");
    }

    sanitized
}

/// Validate a filename against a registered asset type's allowed extensions.
fn validate_filename(
    filename: &str,
    asset_type: &RegisteredAssetType,
) -> Result<String, PluginAssetError> {
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(PluginAssetError::InvalidFilename("Filename too long".to_string()));
    }
    if filename.is_empty() || filename == "." {
        return Err(PluginAssetError::InvalidFilename("Filename cannot be empty".to_string()));
    }
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(PluginAssetError::InvalidFilename(
            "Invalid characters in filename".to_string(),
        ));
    }

    let extension = match filename.rsplit('.').next() {
        Some(ext) if filename.contains('.') => ext.to_lowercase(),
        _ => {
            return Err(PluginAssetError::InvalidFilename(
                "File must have an extension".to_string(),
            ))
        },
    };

    if !asset_type.extensions.iter().any(|e| e.eq_ignore_ascii_case(&extension)) {
        return Err(PluginAssetError::InvalidFormat(format!(
            "Unsupported format: {}. Allowed: {}",
            extension,
            asset_type.extensions.join(", ")
        )));
    }

    Ok(extension)
}

/// Process a directory entry into a [`PluginAsset`].
async fn process_entry(
    path: std::path::PathBuf,
    is_system: bool,
    perms: &RolePermissions,
    asset_type: &RegisteredAssetType,
) -> Option<PluginAsset> {
    if path.is_dir() {
        return None;
    }

    let filename = path.file_name().and_then(|s| s.to_str())?.to_string();
    let extension = path.extension().and_then(|s| s.to_str()).map(str::to_lowercase)?;

    if !asset_type.extensions.iter().any(|e| e.eq_ignore_ascii_case(&extension)) {
        return None;
    }

    let metadata = fs::metadata(&path).await.ok()?;
    let size_bytes = metadata.len();

    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");

    let scope = if is_system { "system" } else { "user" };
    // Build the relative path from the system_dir's parent.
    let base = asset_type.system_dir.parent().unwrap_or(&asset_type.system_dir);
    let asset_path_str = format!("{}/{scope}/{filename}", base.display());

    if !perms.is_asset_allowed(&asset_path_str) {
        debug!("Plugin asset filtered by permissions: {}", asset_path_str);
        return None;
    }

    Some(PluginAsset {
        id: filename,
        name: display_name,
        path: asset_path_str,
        format: extension,
        size_bytes,
        is_system,
        type_id: asset_type.type_id.clone(),
        plugin_id: asset_type.plugin_id.clone(),
    })
}

/// Scan a directory for assets matching a registered type.
async fn scan_directory(
    dir_path: &std::path::Path,
    is_system: bool,
    perms: &RolePermissions,
    asset_type: &RegisteredAssetType,
) -> Result<Vec<PluginAsset>, PluginAssetError> {
    let mut assets = Vec::new();

    if !dir_path.exists() {
        // Not an error — the directory may simply not exist yet (e.g. no user uploads).
        return Ok(assets);
    }

    let mut entries = fs::read_dir(dir_path)
        .await
        .map_err(|e| PluginAssetError::IoError(format!("Failed to read directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| PluginAssetError::IoError(format!("Failed to read entry: {e}")))?
    {
        if let Some(asset) = process_entry(entry.path(), is_system, perms, asset_type).await {
            assets.push(asset);
        }
    }

    Ok(assets)
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// `GET /api/v1/assets/plugin/{type_id}` — list all assets of a plugin type.
async fn list_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(type_id): Path<String>,
) -> Response {
    let perms = get_permissions(&headers, &app_state);

    let Some(asset_type) = app_state.plugin_asset_registry.get(&type_id).await else {
        return (StatusCode::NOT_FOUND, format!("Unknown asset type: {type_id}")).into_response();
    };

    let mut all_assets = Vec::new();

    match scan_directory(&asset_type.system_dir, true, &perms, &asset_type).await {
        Ok(assets) => all_assets.extend(assets),
        Err(e) => {
            error!("Failed to scan system dir for {}: {}", type_id, e);
            return e.into_response();
        },
    }

    match scan_directory(&asset_type.user_dir, false, &perms, &asset_type).await {
        Ok(assets) => all_assets.extend(assets),
        Err(e) => {
            error!("Failed to scan user dir for {}: {}", type_id, e);
            return e.into_response();
        },
    }

    all_assets.sort_by(|a, b| a.name.cmp(&b.name));
    debug!("Listed {} {} assets", all_assets.len(), type_id);
    Json(all_assets).into_response()
}

/// `POST /api/v1/assets/plugin/{type_id}` — upload an asset.
async fn upload_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(type_id): Path<String>,
    mut multipart: Multipart,
) -> Response {
    let perms = get_permissions(&headers, &app_state);

    if !perms.upload_assets {
        return PluginAssetError::Forbidden.into_response();
    }

    let Some(asset_type) = app_state.plugin_asset_registry.get(&type_id).await else {
        return (StatusCode::NOT_FOUND, format!("Unknown asset type: {type_id}")).into_response();
    };

    let field = match multipart.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => {
            return PluginAssetError::InvalidRequest("No file provided".to_string()).into_response()
        },
        Err(e) => {
            return PluginAssetError::InvalidRequest(format!("Failed to read multipart: {e}"))
                .into_response()
        },
    };

    let filename = match field.file_name() {
        Some(name) => sanitize_filename(name),
        None => {
            return PluginAssetError::InvalidRequest("No filename provided".to_string())
                .into_response()
        },
    };
    let extension = match validate_filename(&filename, &asset_type) {
        Ok(ext) => ext,
        Err(e) => return e.into_response(),
    };

    // Ensure user directory exists.
    if let Err(e) = fs::create_dir_all(&asset_type.user_dir).await {
        return PluginAssetError::IoError(format!("Failed to create directory: {e}"))
            .into_response();
    }

    let file_path = asset_type.user_dir.join(&filename);

    // Defense-in-depth: verify the user directory (which now exists after
    // create_dir_all) canonicalizes to where we expect.  We cannot
    // canonicalize `file_path` itself because the file doesn't exist yet.
    // Instead we canonicalize the parent and check the filename is safe.
    {
        let canonical_dir = match asset_type.user_dir.canonicalize() {
            Ok(d) => d,
            Err(e) => {
                return PluginAssetError::IoError(format!("Failed to resolve directory: {e}"))
                    .into_response()
            },
        };
        let target = canonical_dir.join(&filename);
        // After sanitization the filename should never contain path separators,
        // but verify the joined result is still inside the directory.
        if !target.starts_with(&canonical_dir) {
            error!("Upload path escapes user directory: {:?} not in {:?}", target, canonical_dir);
            return PluginAssetError::Forbidden.into_response();
        }
    }

    // Stream to disk with size enforcement.
    match write_upload_to_disk(field, &file_path, asset_type.max_size_bytes).await {
        Ok(written_bytes) => {
            let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
            let display_name = name_without_ext.replace(['_', '-'], " ");
            let base = asset_type.system_dir.parent().unwrap_or(&asset_type.system_dir);
            let relative_path = format!("{}/user/{filename}", base.display());

            info!("Uploaded plugin asset: {} (type: {})", filename, type_id);

            (
                StatusCode::CREATED,
                Json(PluginAsset {
                    id: filename,
                    name: display_name,
                    path: relative_path,
                    format: extension,
                    size_bytes: written_bytes as u64,
                    is_system: false,
                    type_id: asset_type.type_id.clone(),
                    plugin_id: asset_type.plugin_id.clone(),
                }),
            )
                .into_response()
        },
        Err(e) => {
            error!("Failed to upload plugin asset: {}", e);
            e.into_response()
        },
    }
}

/// `DELETE /api/v1/assets/plugin/{type_id}/{id}` — delete a user-uploaded asset.
async fn delete_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((type_id, id)): Path<(String, String)>,
) -> Response {
    let perms = get_permissions(&headers, &app_state);

    if !perms.delete_assets {
        return PluginAssetError::Forbidden.into_response();
    }

    let Some(asset_type) = app_state.plugin_asset_registry.get(&type_id).await else {
        return (StatusCode::NOT_FOUND, format!("Unknown asset type: {type_id}")).into_response();
    };

    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return PluginAssetError::InvalidFilename("Invalid characters in filename".to_string())
            .into_response();
    }

    let file_path = asset_type.user_dir.join(&id);

    // Verify the file is inside the user directory (path traversal protection).
    // Also returns NotFound if the file doesn't exist (avoids TOCTOU with a
    // separate exists() check).
    if let Err(e) = validate_file_in_directory(&file_path, &asset_type.user_dir) {
        return e.into_response();
    }

    if let Err(e) = fs::remove_file(&file_path).await {
        error!("Failed to delete plugin asset file: {}", e);
        return PluginAssetError::IoError(format!("Failed to delete file: {e}")).into_response();
    }

    info!("Deleted plugin asset: {} (type: {})", id, type_id);
    StatusCode::NO_CONTENT.into_response()
}

/// `GET /api/v1/assets/plugin/{type_id}/file/{scope}/{id}` — serve raw file.
async fn serve_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((type_id, scope, id)): Path<(String, String, String)>,
) -> Response {
    use axum::http::header;

    let perms = get_permissions(&headers, &app_state);

    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return PluginAssetError::InvalidFilename("Invalid characters in filename".to_string())
            .into_response();
    }

    if scope != "user" && scope != "system" {
        return PluginAssetError::InvalidFilename(
            "Invalid scope: must be 'user' or 'system'".to_string(),
        )
        .into_response();
    }

    let Some(asset_type) = app_state.plugin_asset_registry.get(&type_id).await else {
        return (StatusCode::NOT_FOUND, format!("Unknown asset type: {type_id}")).into_response();
    };

    let dir = if scope == "system" { &asset_type.system_dir } else { &asset_type.user_dir };
    let file_path = dir.join(&id);

    let base = asset_type.system_dir.parent().unwrap_or(&asset_type.system_dir);
    let asset_path_str = format!("{}/{scope}/{id}", base.display());

    if !perms.is_asset_allowed(&asset_path_str) {
        return PluginAssetError::Forbidden.into_response();
    }

    // Validate extension against registered type before serving.
    let extension =
        file_path.extension().and_then(|e| e.to_str()).map(str::to_lowercase).unwrap_or_default();
    if !asset_type.extensions.iter().any(|e| e.eq_ignore_ascii_case(&extension)) {
        return PluginAssetError::InvalidFormat(format!(
            "File extension '{extension}' is not permitted for asset type '{type_id}'"
        ))
        .into_response();
    }

    // Canonical path validation — also returns NotFound if the file doesn't
    // exist, avoiding a TOCTOU race with a separate exists() check.
    if let Err(e) = validate_file_in_directory(&file_path, dir) {
        return e.into_response();
    }

    let content_type_header = if asset_type.content_type == AssetContentType::Text {
        "text/plain; charset=utf-8"
    } else {
        "application/octet-stream"
    };

    match fs::read(&file_path).await {
        Ok(data) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type_header.to_string()),
                (header::CACHE_CONTROL, "public, must-revalidate".to_string()),
            ],
            data,
        )
            .into_response(),
        Err(e) => {
            error!("Failed to read plugin asset {:?}: {}", file_path, e);
            PluginAssetError::IoError(format!("Failed to read file: {e}")).into_response()
        },
    }
}

/// `PUT /api/v1/assets/plugin/{type_id}/file/{scope}/{id}` — update text file in-place.
async fn update_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((type_id, scope, id)): Path<(String, String, String)>,
    body: String,
) -> Response {
    let perms = get_permissions(&headers, &app_state);

    if !perms.upload_assets {
        return PluginAssetError::Forbidden.into_response();
    }

    if scope != "user" {
        return PluginAssetError::Forbidden.into_response();
    }

    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return PluginAssetError::InvalidFilename("Invalid characters in filename".to_string())
            .into_response();
    }

    let Some(asset_type) = app_state.plugin_asset_registry.get(&type_id).await else {
        return (StatusCode::NOT_FOUND, format!("Unknown asset type: {type_id}")).into_response();
    };

    if asset_type.content_type != AssetContentType::Text {
        return PluginAssetError::InvalidRequest(
            "Only text asset types support in-place editing".to_string(),
        )
        .into_response();
    }

    let extension = match validate_filename(&id, &asset_type) {
        Ok(ext) => ext,
        Err(e) => return e.into_response(),
    };

    let file_path = asset_type.user_dir.join(&id);

    // Canonical path validation — also returns NotFound if the file doesn't
    // exist, avoiding a TOCTOU race with a separate exists() check.
    if let Err(e) = validate_file_in_directory(&file_path, &asset_type.user_dir) {
        return e.into_response();
    }

    if body.len() > asset_type.max_size_bytes {
        return PluginAssetError::FileTooLarge(asset_type.max_size_bytes).into_response();
    }

    if let Err(e) = fs::write(&file_path, body.as_bytes()).await {
        error!("Failed to write plugin asset {:?}: {}", file_path, e);
        return PluginAssetError::IoError(format!("Failed to write file: {e}")).into_response();
    }

    let metadata = match fs::metadata(&file_path).await {
        Ok(m) => m,
        Err(e) => {
            return PluginAssetError::IoError(format!("Failed to read metadata: {e}"))
                .into_response()
        },
    };

    let name_without_ext = id.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");
    let base = asset_type.system_dir.parent().unwrap_or(&asset_type.system_dir);
    let relative_path = format!("{}/user/{id}", base.display());

    info!("Updated plugin asset: {} (type: {})", id, type_id);

    Json(PluginAsset {
        id,
        name: display_name,
        path: relative_path,
        format: extension,
        size_bytes: metadata.len(),
        is_system: false,
        type_id: asset_type.type_id.clone(),
        plugin_id: asset_type.plugin_id.clone(),
    })
    .into_response()
}

// ── Asset Type Discovery ─────────────────────────────────────────────────────

/// `GET /api/v1/asset-types` — returns all registered asset types (core + plugin).
pub async fn list_asset_types_handler(
    State(app_state): State<Arc<AppState>>,
) -> Json<Vec<AssetTypeInfo>> {
    let mut types = vec![
        // Core asset types — always present.
        AssetTypeInfo {
            type_id: "audio".to_string(),
            label: "Audio".to_string(),
            source: "core".to_string(),
            plugin_id: None,
            node_kind: None,
            node_param: None,
            extensions: vec![
                "opus".to_string(),
                "ogg".to_string(),
                "flac".to_string(),
                "mp3".to_string(),
                "wav".to_string(),
            ],
            icon_hint: "music".to_string(),
            editable: false,
        },
        AssetTypeInfo {
            type_id: "images".to_string(),
            label: "Images".to_string(),
            source: "core".to_string(),
            plugin_id: None,
            node_kind: None,
            node_param: None,
            extensions: vec![
                "png".to_string(),
                "jpg".to_string(),
                "jpeg".to_string(),
                "webp".to_string(),
                "gif".to_string(),
            ],
            icon_hint: "image".to_string(),
            editable: false,
        },
        AssetTypeInfo {
            type_id: "fonts".to_string(),
            label: "Fonts".to_string(),
            source: "core".to_string(),
            plugin_id: None,
            node_kind: None,
            node_param: None,
            extensions: vec!["ttf".to_string(), "otf".to_string(), "woff2".to_string()],
            icon_hint: "type".to_string(),
            editable: false,
        },
    ];

    // Append plugin-registered types.
    for reg in app_state.plugin_asset_registry.all().await {
        types.push(AssetTypeInfo {
            type_id: reg.type_id,
            label: reg.label,
            source: "plugin".to_string(),
            plugin_id: Some(reg.plugin_id),
            node_kind: Some(reg.node_kind),
            node_param: reg.node_param,
            extensions: reg.extensions,
            icon_hint: reg.icon_hint.unwrap_or_else(|| "file".to_string()),
            editable: reg.content_type == AssetContentType::Text,
        });
    }

    Json(types)
}

// ── Upload helper ────────────────────────────────────────────────────────────

async fn write_upload_to_disk(
    mut field: axum::extract::multipart::Field<'_>,
    file_path: &std::path::Path,
    max_size: usize,
) -> Result<usize, PluginAssetError> {
    use tokio::fs::OpenOptions;

    let mut file =
        OpenOptions::new().create_new(true).write(true).open(file_path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                PluginAssetError::FileExists(
                    file_path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string(),
                )
            } else {
                PluginAssetError::IoError(format!("Failed to create file: {e}"))
            }
        })?;

    let mut total_bytes: usize = 0;
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                total_bytes = total_bytes.saturating_add(chunk.len());
                if total_bytes > max_size {
                    let _ = fs::remove_file(file_path).await;
                    return Err(PluginAssetError::FileTooLarge(max_size));
                }

                if let Err(e) = file.write_all(&chunk).await {
                    let _ = fs::remove_file(file_path).await;
                    return Err(PluginAssetError::IoError(format!("Failed to write file: {e}")));
                }
            },
            Ok(None) => break,
            Err(e) => {
                let _ = fs::remove_file(file_path).await;
                return Err(PluginAssetError::InvalidRequest(format!(
                    "Failed to read upload stream: {e}"
                )));
            },
        }
    }

    Ok(total_bytes)
}

fn validate_file_in_directory(
    file_path: &std::path::Path,
    expected_dir: &std::path::Path,
) -> Result<(), PluginAssetError> {
    let canonical = file_path.canonicalize().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            PluginAssetError::NotFound(
                file_path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
            )
        } else {
            PluginAssetError::IoError(format!("Failed to resolve file path: {e}"))
        }
    })?;

    let canonical_dir = expected_dir
        .canonicalize()
        .map_err(|_| PluginAssetError::IoError("Failed to resolve directory".to_string()))?;

    if !canonical.starts_with(&canonical_dir) {
        error!("Attempt to access asset outside expected directory: {:?}", canonical);
        return Err(PluginAssetError::Forbidden);
    }

    Ok(())
}

// ── Router ───────────────────────────────────────────────────────────────────

/// Create the router for plugin asset endpoints and the asset-types discovery
/// endpoint.
///
/// The body limit is set to a generous default; individual type limits are
/// enforced inside the upload handler.
pub fn plugin_assets_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/asset-types", get(list_asset_types_handler))
        .route(
            "/api/v1/assets/plugin/{type_id}",
            get(list_handler).post(upload_handler).layer(DefaultBodyLimit::max(100 * 1024 * 1024)), // 100 MiB outer limit
        )
        .route("/api/v1/assets/plugin/{type_id}/{id}", delete(delete_handler))
        .route(
            "/api/v1/assets/plugin/{type_id}/file/{scope}/{id}",
            get(serve_handler).put(update_handler).layer(DefaultBodyLimit::max(100 * 1024 * 1024)),
        )
}

// ── Manifest loading helper ──────────────────────────────────────────────────

/// Attempt to read a `plugin.yml` from the same directory as a plugin library.
///
/// Used when loading local (non-marketplace) plugins from disk.  The manifest
/// is parsed as a [`PluginManifest`] to extract `assets` declarations.
///
/// Searches for the manifest in two locations:
/// 1. `plugin.yml` / `plugin.yaml` in the same directory as the `.so` file
///    (works when the plugin is in its source tree, e.g. `plugins/native/slint/`).
/// 2. `{stem}.plugin.yml` next to the `.so` file (works with the flat layout
///    produced by `just copy-plugins-native`, e.g. `.plugins/native/slint.plugin.yml`).
pub fn read_local_plugin_manifest(
    library_path: &std::path::Path,
) -> Option<crate::marketplace::PluginManifest> {
    let dir = library_path.parent()?;

    // Derive the plugin name from the library filename:
    //   libslint.so  ->  slint
    //   libgain_plugin_native.so  ->  gain_plugin_native
    let stem = library_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.strip_prefix("lib").unwrap_or(s));

    // Build candidate paths: generic names first, then stem-based.
    let mut candidates: Vec<std::path::PathBuf> =
        vec![dir.join("plugin.yml"), dir.join("plugin.yaml")];

    if let Some(stem) = stem {
        candidates.push(dir.join(format!("{stem}.plugin.yml")));
        candidates.push(dir.join(format!("{stem}.plugin.yaml")));
    }

    for manifest_path in &candidates {
        if manifest_path.exists() {
            match std::fs::read_to_string(manifest_path) {
                Ok(contents) => match serde_saphyr::from_str(&contents) {
                    Ok(manifest) => return Some(manifest),
                    Err(e) => {
                        warn!(
                            path = %manifest_path.display(),
                            error = %e,
                            "Failed to parse plugin manifest"
                        );
                    },
                },
                Err(e) => {
                    warn!(
                        path = %manifest_path.display(),
                        error = %e,
                        "Failed to read plugin manifest"
                    );
                },
            }
        }
    }

    None
}

// ── Error type ───────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum PluginAssetError {
    IoError(String),
    InvalidFilename(String),
    InvalidFormat(String),
    InvalidRequest(String),
    FileTooLarge(usize),
    FileExists(String),
    NotFound(String),
    Forbidden,
}

impl IntoResponse for PluginAssetError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::IoError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            Self::InvalidFilename(msg) | Self::InvalidFormat(msg) | Self::InvalidRequest(msg) => {
                (StatusCode::BAD_REQUEST, msg)
            },
            Self::FileTooLarge(max) => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!("File too large. Maximum size: {max} bytes"),
            ),
            Self::FileExists(filename) => {
                (StatusCode::CONFLICT, format!("File already exists: {filename}"))
            },
            Self::NotFound(id) => (StatusCode::NOT_FOUND, format!("Asset not found: {id}")),
            Self::Forbidden => (StatusCode::FORBIDDEN, "Insufficient permissions".to_string()),
        };

        (status, message).into_response()
    }
}

impl std::fmt::Display for PluginAssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IoError(msg) => write!(f, "IO error: {msg}"),
            Self::InvalidFilename(msg) => write!(f, "Invalid filename: {msg}"),
            Self::InvalidFormat(msg) => write!(f, "Invalid format: {msg}"),
            Self::InvalidRequest(msg) => write!(f, "Invalid request: {msg}"),
            Self::FileTooLarge(max) => write!(f, "File too large (max: {max} bytes)"),
            Self::FileExists(filename) => write!(f, "File exists: {filename}"),
            Self::NotFound(id) => write!(f, "Not found: {id}"),
            Self::Forbidden => write!(f, "Forbidden"),
        }
    }
}

impl std::error::Error for PluginAssetError {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to build a minimal `RegisteredAssetType` for testing.
    fn test_asset_type(extensions: &[&str]) -> RegisteredAssetType {
        RegisteredAssetType {
            type_id: "test".to_string(),
            plugin_id: "test-plugin".to_string(),
            node_kind: "plugin::native::test".to_string(),
            label: "Test".to_string(),
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: PathBuf::from("samples/test/system"),
            user_dir: PathBuf::from("samples/test/user"),
        }
    }

    // ── sanitize_filename ────────────────────────────────────────────────

    #[test]
    fn sanitize_keeps_safe_chars() {
        assert_eq!(sanitize_filename("hello_world-1.slint"), "hello_world-1.slint");
    }

    #[test]
    fn sanitize_replaces_dangerous_chars() {
        assert_eq!(sanitize_filename("../../etc/passwd"), ".._.._etc_passwd");
    }

    #[test]
    fn sanitize_replaces_slashes() {
        assert_eq!(sanitize_filename("path/to\\file.txt"), "path_to_file.txt");
    }

    #[test]
    fn sanitize_replaces_spaces_and_special() {
        assert_eq!(sanitize_filename("my file (1).txt"), "my_file__1_.txt");
    }

    #[test]
    fn sanitize_rejects_dot() {
        assert_eq!(sanitize_filename("."), "_invalid_");
    }

    #[test]
    fn sanitize_rejects_dotdot() {
        assert_eq!(sanitize_filename(".."), "_invalid_");
    }

    #[test]
    fn sanitize_handles_empty_string() {
        assert_eq!(sanitize_filename(""), "_invalid_");
    }

    #[test]
    fn sanitize_handles_null_bytes() {
        // Null bytes are not in the allow-list, so they become underscores.
        assert_eq!(sanitize_filename("file\0.txt"), "file_.txt");
    }

    #[test]
    fn sanitize_handles_unicode() {
        // Non-ASCII chars are replaced with underscores.
        assert_eq!(sanitize_filename("café.txt"), "caf_.txt");
    }

    #[test]
    fn sanitize_preserves_hidden_file() {
        assert_eq!(sanitize_filename(".hidden.txt"), ".hidden.txt");
    }

    // ── validate_filename ────────────────────────────────────────────────

    #[test]
    fn validate_accepts_valid_extension() {
        let at = test_asset_type(&["slint", "txt"]);
        assert_eq!(validate_filename("test.slint", &at).unwrap(), "slint");
    }

    #[test]
    fn validate_case_insensitive_extension() {
        let at = test_asset_type(&["slint"]);
        assert_eq!(validate_filename("test.SLINT", &at).unwrap(), "slint");
    }

    #[test]
    fn validate_rejects_dot() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename(".", &at).is_err());
    }

    #[test]
    fn validate_rejects_dotdot() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename("..", &at).is_err());
    }

    #[test]
    fn validate_rejects_path_traversal() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename("../../etc/passwd", &at).is_err());
    }

    #[test]
    fn validate_rejects_slash() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename("sub/file.txt", &at).is_err());
    }

    #[test]
    fn validate_rejects_backslash() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename("sub\\file.txt", &at).is_err());
    }

    #[test]
    fn validate_rejects_no_extension() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename("noextension", &at).is_err());
    }

    #[test]
    fn validate_rejects_wrong_extension() {
        let at = test_asset_type(&["slint"]);
        assert!(validate_filename("test.exe", &at).is_err());
    }

    #[test]
    fn validate_rejects_too_long_filename() {
        let at = test_asset_type(&["txt"]);
        let long_name = format!("{}.txt", "a".repeat(300));
        assert!(validate_filename(&long_name, &at).is_err());
    }

    #[test]
    fn validate_rejects_empty() {
        let at = test_asset_type(&["txt"]);
        assert!(validate_filename("", &at).is_err());
    }

    // ── validate_file_in_directory ───────────────────────────────────────

    #[test]
    fn validate_file_in_dir_accepts_child() {
        let dir = std::env::temp_dir();
        let child = dir.join("test_child.txt");
        std::fs::write(&child, "test").unwrap();
        assert!(validate_file_in_directory(&child, &dir).is_ok());
        std::fs::remove_file(&child).unwrap();
    }

    #[test]
    fn validate_file_in_dir_rejects_outside() {
        let dir = std::env::temp_dir().join("plugin_asset_test_subdir");
        std::fs::create_dir_all(&dir).unwrap();
        // Create a file one level above the expected directory.
        let outside = std::env::temp_dir().join("outside_test.txt");
        std::fs::write(&outside, "test").unwrap();
        assert!(validate_file_in_directory(&outside, &dir).is_err());
        std::fs::remove_file(&outside).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    // ── register validation ─────────────────────────────────────────────

    #[tokio::test]
    async fn register_rejects_empty_type_id() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: String::new(),
            label: "Empty".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        registry.register("test", "plugin::native::test", &[spec]).await;
        assert!(registry.all().await.is_empty());
    }

    #[tokio::test]
    async fn register_rejects_traversal_type_id() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "../audio".to_string(),
            label: "Bad".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        registry.register("test", "plugin::native::test", &[spec]).await;
        assert!(registry.all().await.is_empty());
    }

    #[tokio::test]
    async fn register_rejects_absolute_system_dir() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "test".to_string(),
            label: "Bad".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: Some("/etc/secrets".to_string()),
        };
        registry.register("test", "plugin::native::test", &[spec]).await;
        assert!(registry.all().await.is_empty());
    }

    #[tokio::test]
    async fn register_rejects_dotdot_system_dir() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "test".to_string(),
            label: "Bad".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: Some("../../etc".to_string()),
        };
        registry.register("test", "plugin::native::test", &[spec]).await;
        assert!(registry.all().await.is_empty());
    }

    #[tokio::test]
    async fn register_accepts_valid_spec() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "slint".to_string(),
            label: "Slint Files".to_string(),
            extensions: vec!["slint".to_string()],
            max_size_bytes: 1_048_576,
            content_type: AssetContentType::Text,
            icon_hint: Some("code".to_string()),
            node_param: Some("slint_file".to_string()),
            system_dir: Some("samples/slint/system".to_string()),
        };
        registry.register("slint", "plugin::native::slint", &[spec]).await;
        assert_eq!(registry.all().await.len(), 1);
    }

    #[tokio::test]
    async fn register_accepts_dotdot_substring_in_dirname() {
        // "my..assets" contains ".." as a substring but is not a ParentDir component.
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "test".to_string(),
            label: "Test".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: Some("samples/my..assets/system".to_string()),
        };
        registry.register("test", "plugin::native::test", &[spec]).await;
        assert_eq!(registry.all().await.len(), 1);
    }
}
