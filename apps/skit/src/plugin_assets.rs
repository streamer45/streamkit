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
use streamkit_api::{AssetTypeInfo, AssetTypeSource, PluginAsset};

// Security limits
const MAX_FILENAME_LENGTH: usize = 255;
/// Hard cap on `max_size_bytes` for any plugin asset type (100 MiB).
/// Prevents memory exhaustion when `serve_handler` reads files into memory.
const MAX_ASSET_SIZE_BYTES: usize = 100 * 1024 * 1024;

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

/// Thread-safe registry of plugin-declared asset types.
///
/// Stored in [`AppState`] and shared across handlers.  Uses an `RwLock` so
/// reads (listing, serving) don't block each other.
///
/// A separate sync cache of permission patterns is maintained so the
/// permission layer can query it without an async context (see
/// [`registered_permission_patterns`](Self::registered_permission_patterns)).
#[derive(Debug, Default, Clone)]
pub struct PluginAssetRegistry {
    inner: Arc<RwLock<HashMap<String, RegisteredAssetType>>>,
    /// Sync-accessible `(system_glob, user_glob)` pairs for permission augmentation.
    permission_patterns: Arc<std::sync::RwLock<Vec<(String, String)>>>,
}

impl PluginAssetRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            permission_patterns: Arc::new(std::sync::RwLock::new(Vec::new())),
        }
    }

    /// Register asset types declared by a plugin.
    ///
    /// `plugin_id` and `node_kind` come from the plugin manifest.
    /// Returns the number of types successfully registered; invalid specs
    /// (bad `type_id` or `system_dir` with path-traversal components) are
    /// logged and skipped.
    ///
    /// `max_size_bytes` is capped at [`MAX_ASSET_SIZE_BYTES`] to prevent
    /// memory exhaustion when serving files.
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
                if streamkit_core::path_helpers::has_path_traversal(path) {
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
            let user_dir = system_dir.parent().filter(|p| !p.as_os_str().is_empty()).map_or_else(
                || PathBuf::from(format!("samples/{}/user", spec.type_id)),
                |p| p.join("user"),
            );

            let registered = RegisteredAssetType {
                type_id: spec.type_id.clone(),
                plugin_id: plugin_id.to_string(),
                node_kind: node_kind.to_string(),
                label: spec.label.clone(),
                extensions: spec.extensions.clone(),
                max_size_bytes: spec.max_size_bytes.min(MAX_ASSET_SIZE_BYTES),
                content_type: spec.content_type.clone(),
                icon_hint: spec.icon_hint.clone(),
                node_param: spec.node_param.clone(),
                system_dir,
                user_dir,
            };

            // Reject if another plugin already owns this type_id.
            if let Some(existing) = map.get(&spec.type_id) {
                if existing.plugin_id != plugin_id {
                    warn!(
                        type_id = %spec.type_id,
                        existing_plugin = %existing.plugin_id,
                        new_plugin = %plugin_id,
                        "Skipping plugin asset type: type_id already registered by a different plugin"
                    );
                    continue;
                }
            }

            info!(
                type_id = %spec.type_id,
                plugin_id = %plugin_id,
                extensions = ?spec.extensions,
                "Registered plugin asset type"
            );

            map.insert(spec.type_id.clone(), registered);
        }

        // Rebuild the sync permission patterns cache from the authoritative map.
        if let Ok(mut patterns) = self.permission_patterns.write() {
            *patterns = map
                .values()
                .map(|r| {
                    (format!("{}/*", r.system_dir.display()), format!("{}/*", r.user_dir.display()))
                })
                .collect();
        }
    }

    /// Remove all asset types owned by a plugin.
    ///
    /// Called when a plugin is unloaded or deleted so the CRUD endpoints stop
    /// serving stale types.  User-uploaded files are left in place.
    pub async fn unregister_plugin(&self, plugin_id: &str) {
        let mut map = self.inner.write().await;
        let before = map.len();
        map.retain(|_, v| v.plugin_id != plugin_id);
        let removed = before - map.len();

        // Rebuild the sync permission patterns cache.
        if let Ok(mut patterns) = self.permission_patterns.write() {
            *patterns = map
                .values()
                .map(|r| {
                    (format!("{}/*", r.system_dir.display()), format!("{}/*", r.user_dir.display()))
                })
                .collect();
        }

        drop(map);
        if removed > 0 {
            info!(plugin_id = %plugin_id, removed, "Unregistered plugin asset types");
        }
    }

    /// Returns permission glob patterns for all registered plugin asset types.
    ///
    /// Each entry is a `(system_glob, user_glob)` pair derived from the actual
    /// `system_dir` and `user_dir` of each registered type — not hardcoded to
    /// `samples/{type_id}/`.  Used by the permission layer to dynamically
    /// augment role permissions without broad wildcards.
    pub fn registered_permission_patterns(&self) -> Vec<(String, String)> {
        self.permission_patterns.read().map_or_else(|_| Vec::new(), |p| p.clone())
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

    let system_dir = app_state.asset_root.join(&asset_type.system_dir);
    let user_dir = app_state.asset_root.join(&asset_type.user_dir);

    match scan_directory(&system_dir, true, &perms, &asset_type).await {
        Ok(assets) => all_assets.extend(assets),
        Err(e) => {
            error!("Failed to scan system dir for {}: {}", type_id, e);
            return e.into_response();
        },
    }

    match scan_directory(&user_dir, false, &perms, &asset_type).await {
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
    let user_dir = app_state.asset_root.join(&asset_type.user_dir);
    if let Err(e) = fs::create_dir_all(&user_dir).await {
        return PluginAssetError::IoError(format!("Failed to create directory: {e}"))
            .into_response();
    }

    let file_path = user_dir.join(&filename);

    // Defense-in-depth: verify the user directory (which now exists after
    // create_dir_all) canonicalizes to where we expect.  We cannot
    // canonicalize `file_path` itself because the file doesn't exist yet.
    // Instead we canonicalize the parent and check the filename is safe.
    {
        let canonical_dir = match user_dir.canonicalize() {
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

    let user_dir = app_state.asset_root.join(&asset_type.user_dir);
    let file_path = user_dir.join(&id);

    // Verify the file is inside the user directory (path traversal protection).
    // Also returns NotFound if the file doesn't exist (avoids TOCTOU with a
    // separate exists() check).
    if let Err(e) = validate_file_in_directory(&file_path, &user_dir) {
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

    let dir = if scope == "system" {
        app_state.asset_root.join(&asset_type.system_dir)
    } else {
        app_state.asset_root.join(&asset_type.user_dir)
    };
    let file_path = dir.join(&id);

    let base = asset_type.system_dir.parent().unwrap_or(&asset_type.system_dir);
    let asset_path_str = format!("{}/{scope}/{id}", base.display());

    if !perms.is_asset_allowed(&asset_path_str) {
        return PluginAssetError::Forbidden.into_response();
    }

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
    if let Err(e) = validate_file_in_directory(&file_path, &dir) {
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
                (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{id}\"")),
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

    let user_dir = app_state.asset_root.join(&asset_type.user_dir);
    let file_path = user_dir.join(&id);

    // Canonical path validation — also returns NotFound if the file doesn't
    // exist, avoiding a TOCTOU race with a separate exists() check.
    if let Err(e) = validate_file_in_directory(&file_path, &user_dir) {
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

/// `GET /api/v1/asset-types` — returns all registered asset types (core + plugin).
pub async fn list_asset_types_handler(
    State(app_state): State<Arc<AppState>>,
) -> Json<Vec<AssetTypeInfo>> {
    let mut types = vec![
        // Core asset types — always present.
        AssetTypeInfo {
            type_id: "audio".to_string(),
            label: "Audio".to_string(),
            source: AssetTypeSource::Core,
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
            type_id: "image".to_string(),
            label: "Images".to_string(),
            source: AssetTypeSource::Core,
            plugin_id: None,
            node_kind: None,
            node_param: None,
            extensions: vec![
                "png".to_string(),
                "jpg".to_string(),
                "jpeg".to_string(),
                "webp".to_string(),
                "gif".to_string(),
                "svg".to_string(),
                "svgz".to_string(),
            ],
            icon_hint: "image".to_string(),
            editable: false,
        },
        AssetTypeInfo {
            type_id: "font".to_string(),
            label: "Fonts".to_string(),
            source: AssetTypeSource::Core,
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
            source: AssetTypeSource::Plugin,
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

    // Flush pending writes — tokio::fs::File::write_all returns as soon as
    // data is copied to an internal buffer and a blocking write is spawned,
    // so the last write may still be in-flight when the File is dropped.
    if let Err(e) = file.flush().await {
        let _ = fs::remove_file(file_path).await;
        return Err(PluginAssetError::IoError(format!("Failed to flush file: {e}")));
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
            // 10 MiB — update_handler buffers body: String in memory before
            // checking per-type max_size_bytes (typically 1 MiB).  Keep this
            // well below the 100 MiB multipart upload limit to bound memory.
            get(serve_handler).put(update_handler).layer(DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
}

/// Attempt to read a `plugin.yml` from the same directory as a plugin library.
///
/// Used when loading local (non-marketplace) plugins from disk.  The manifest
/// is parsed as a [`PluginManifest`] to extract `assets` declarations.
///
/// Searches for the manifest in two locations:
/// 1. `plugin.yml` / `plugin.yaml` in the same directory as the `.so` file
///    (works with the directory-per-plugin layout produced by
///    `just copy-plugins-native`, e.g. `.plugins/native/slint/plugin.yml`).
/// 2. `{stem}.plugin.yml` / `{stem}.plugin.yaml` next to the `.so` file
///    (fallback for any non-standard layouts).
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn test_asset_type(extensions: &[&str]) -> RegisteredAssetType {
        RegisteredAssetType {
            type_id: "test".to_string(),
            plugin_id: "test-plugin".to_string(),
            node_kind: "plugin::native::test".to_string(),
            label: "Test".to_string(),
            extensions: extensions.iter().copied().map(String::from).collect(),
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: PathBuf::from("samples/test/system"),
            user_dir: PathBuf::from("samples/test/user"),
        }
    }

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

    #[tokio::test]
    async fn unregister_removes_all_types_for_plugin() {
        let registry = PluginAssetRegistry::new();
        let specs = vec![
            PluginAssetSpec {
                type_id: "alpha".to_string(),
                label: "Alpha".to_string(),
                extensions: vec!["a".to_string()],
                max_size_bytes: 1024,
                content_type: AssetContentType::Binary,
                icon_hint: None,
                node_param: None,
                system_dir: None,
            },
            PluginAssetSpec {
                type_id: "beta".to_string(),
                label: "Beta".to_string(),
                extensions: vec!["b".to_string()],
                max_size_bytes: 1024,
                content_type: AssetContentType::Binary,
                icon_hint: None,
                node_param: None,
                system_dir: None,
            },
        ];
        registry.register("myplugin", "plugin::native::myplugin", &specs).await;
        assert_eq!(registry.all().await.len(), 2);

        registry.unregister_plugin("myplugin").await;
        assert!(registry.all().await.is_empty());
    }

    #[tokio::test]
    async fn unregister_leaves_other_plugins_intact() {
        let registry = PluginAssetRegistry::new();
        let spec_a = PluginAssetSpec {
            type_id: "a".to_string(),
            label: "A".to_string(),
            extensions: vec!["a".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        let spec_b = PluginAssetSpec {
            type_id: "b".to_string(),
            label: "B".to_string(),
            extensions: vec!["b".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        registry.register("plugin-a", "plugin::native::a", &[spec_a]).await;
        registry.register("plugin-b", "plugin::native::b", &[spec_b]).await;
        assert_eq!(registry.all().await.len(), 2);

        registry.unregister_plugin("plugin-a").await;
        let remaining = registry.all().await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].plugin_id, "plugin-b");
    }

    #[tokio::test]
    async fn register_skips_collision_from_different_plugin() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "shared".to_string(),
            label: "Shared".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        // First plugin registers successfully.
        registry.register("plugin-a", "plugin::native::a", std::slice::from_ref(&spec)).await;
        assert_eq!(registry.all().await.len(), 1);
        assert_eq!(registry.all().await[0].plugin_id, "plugin-a");

        // Second plugin with same type_id is rejected.
        registry.register("plugin-b", "plugin::native::b", &[spec]).await;
        assert_eq!(registry.all().await.len(), 1);
        assert_eq!(registry.all().await[0].plugin_id, "plugin-a");
    }

    #[tokio::test]
    async fn register_allows_same_plugin_reregistration() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "mine".to_string(),
            label: "Mine".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        registry.register("plugin-a", "plugin::native::a", std::slice::from_ref(&spec)).await;
        // Re-registering from the same plugin should succeed (idempotent).
        registry.register("plugin-a", "plugin::native::a", std::slice::from_ref(&spec)).await;
        assert_eq!(registry.all().await.len(), 1);
    }

    #[tokio::test]
    async fn register_caps_max_size_bytes() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "big".to_string(),
            label: "Big".to_string(),
            extensions: vec!["bin".to_string()],
            max_size_bytes: 1_000_000_000, // 1 GB — exceeds the 100 MiB cap
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        registry.register("test", "plugin::native::test", &[spec]).await;
        let types = registry.all().await;
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].max_size_bytes, MAX_ASSET_SIZE_BYTES);
    }

    #[tokio::test]
    async fn permission_patterns_track_register_and_unregister() {
        let registry = PluginAssetRegistry::new();
        assert!(registry.registered_permission_patterns().is_empty());

        let spec = PluginAssetSpec {
            type_id: "test".to_string(),
            label: "Test".to_string(),
            extensions: vec!["txt".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: None,
        };
        registry.register("myplugin", "plugin::native::myplugin", &[spec]).await;
        let patterns = registry.registered_permission_patterns();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].0, "samples/test/system/*");
        assert_eq!(patterns[0].1, "samples/test/user/*");

        registry.unregister_plugin("myplugin").await;
        assert!(registry.registered_permission_patterns().is_empty());
    }

    #[tokio::test]
    async fn permission_patterns_respect_custom_system_dir() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "custom".to_string(),
            label: "Custom".to_string(),
            extensions: vec!["dat".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: Some("data/custom/system".to_string()),
        };
        registry.register("myplugin", "plugin::native::myplugin", &[spec]).await;
        let patterns = registry.registered_permission_patterns();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].0, "data/custom/system/*");
        assert_eq!(patterns[0].1, "data/custom/user/*");
    }

    #[tokio::test]
    async fn single_component_system_dir_falls_back_to_default_user_dir() {
        let registry = PluginAssetRegistry::new();
        let spec = PluginAssetSpec {
            type_id: "quirky".to_string(),
            label: "Quirky".to_string(),
            extensions: vec!["bin".to_string()],
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            // Single-component path — parent() would return "".
            system_dir: Some("system".to_string()),
        };
        registry.register("myplugin", "plugin::native::myplugin", &[spec]).await;
        let patterns = registry.registered_permission_patterns();
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].0, "system/*");
        // user_dir should fall back to `samples/quirky/user`, not bare `user`.
        assert_eq!(patterns[0].1, "samples/quirky/user/*");
    }

    #[test]
    fn plugin_asset_error_display_messages() {
        assert_eq!(PluginAssetError::IoError("boom".to_string()).to_string(), "IO error: boom");
        assert_eq!(
            PluginAssetError::InvalidFilename("bad".to_string()).to_string(),
            "Invalid filename: bad"
        );
        assert_eq!(
            PluginAssetError::InvalidFormat("nope".to_string()).to_string(),
            "Invalid format: nope"
        );
        assert_eq!(
            PluginAssetError::InvalidRequest("missing".to_string()).to_string(),
            "Invalid request: missing"
        );
        assert_eq!(
            PluginAssetError::FileTooLarge(42).to_string(),
            "File too large (max: 42 bytes)"
        );
        assert_eq!(
            PluginAssetError::FileExists("a.txt".to_string()).to_string(),
            "File exists: a.txt"
        );
        assert_eq!(
            PluginAssetError::NotFound("missing".to_string()).to_string(),
            "Not found: missing"
        );
        assert_eq!(PluginAssetError::Forbidden.to_string(), "Forbidden");
    }

    #[tokio::test]
    async fn plugin_asset_error_maps_to_expected_status_codes() {
        for (err, expected) in [
            (PluginAssetError::IoError("x".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (PluginAssetError::InvalidFilename("x".into()), StatusCode::BAD_REQUEST),
            (PluginAssetError::InvalidFormat("x".into()), StatusCode::BAD_REQUEST),
            (PluginAssetError::InvalidRequest("x".into()), StatusCode::BAD_REQUEST),
            (PluginAssetError::FileTooLarge(1), StatusCode::PAYLOAD_TOO_LARGE),
            (PluginAssetError::FileExists("a".into()), StatusCode::CONFLICT),
            (PluginAssetError::NotFound("a".into()), StatusCode::NOT_FOUND),
            (PluginAssetError::Forbidden, StatusCode::FORBIDDEN),
        ] {
            assert_eq!(err.into_response().status(), expected);
        }
    }
}

#[cfg(test)]
// `unwrap`/`expect` in tests fail fast on setup mistakes; production policy enforced elsewhere.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod handler_tests {
    //! Integration-style tests that exercise the public Axum router built
    //! by [`plugin_assets_router`].
    //!
    //! These tests bypass [`PluginAssetRegistry::register`] (which only
    //! accepts relative `system_dir` values) and insert
    //! [`RegisteredAssetType`] values pointing directly at a `TempDir`,
    //! so each test runs against a fresh filesystem with no CWD
    //! manipulation.

    use super::*;
    use crate::config::{AuthMode, Config};
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use std::path::Path;
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn make_state() -> Arc<AppState> {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        crate::server::create_app_state(config, None)
    }

    fn make_state_with_asset_root(root: std::path::PathBuf) -> Arc<AppState> {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        config.asset_root = Some(root);
        crate::server::create_app_state(config, None)
    }

    fn make_viewer_state() -> Arc<AppState> {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        config.permissions.default_role = "viewer".to_string();
        crate::server::create_app_state(config, None)
    }

    /// Build a `RegisteredAssetType` whose directories live under the
    /// supplied root.  Both system and user directories are created on
    /// disk so handlers that canonicalize them succeed without first
    /// running upload_handler.
    fn registered_type(
        root: &Path,
        type_id: &str,
        plugin_id: &str,
        extensions: &[&str],
        content_type: AssetContentType,
        max_size_bytes: usize,
    ) -> RegisteredAssetType {
        let system_dir = root.join(type_id).join("system");
        let user_dir = root.join(type_id).join("user");
        std::fs::create_dir_all(&system_dir).unwrap();
        std::fs::create_dir_all(&user_dir).unwrap();
        RegisteredAssetType {
            type_id: type_id.to_string(),
            plugin_id: plugin_id.to_string(),
            node_kind: format!("plugin::native::{plugin_id}"),
            label: type_id.to_string(),
            extensions: extensions.iter().copied().map(String::from).collect(),
            max_size_bytes,
            content_type,
            icon_hint: None,
            node_param: None,
            system_dir,
            user_dir,
        }
    }

    async fn install_type(state: &Arc<AppState>, registered: RegisteredAssetType) {
        let mut map = state.plugin_asset_registry.inner.write().await;
        map.insert(registered.type_id.clone(), registered);
    }

    fn build_multipart_body(
        boundary: &str,
        field_name: &str,
        filename: Option<&str>,
        bytes: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(name) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{field_name}\"; \
                     filename=\"{name}\"\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{field_name}\"\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    fn multipart_request(
        method: Method,
        uri: &str,
        boundary: &str,
        body: Vec<u8>,
    ) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .header(header::CONTENT_TYPE, format!("multipart/form-data; boundary={boundary}"))
            .body(Body::from(body))
            .unwrap()
    }

    async fn body_bytes(body: Body) -> Vec<u8> {
        to_bytes(body, 32 * 1024 * 1024).await.unwrap().to_vec()
    }

    async fn body_string(body: Body) -> String {
        String::from_utf8(body_bytes(body).await).unwrap()
    }

    /// Build a minimal `RegisteredAssetType` for tests of pure helpers
    /// (process_entry, scan_directory) that don't need a real AppState.
    fn test_asset_type(extensions: &[&str]) -> RegisteredAssetType {
        RegisteredAssetType {
            type_id: "test".to_string(),
            plugin_id: "test-plugin".to_string(),
            node_kind: "plugin::native::test".to_string(),
            label: "Test".to_string(),
            extensions: extensions.iter().copied().map(String::from).collect(),
            max_size_bytes: 1024,
            content_type: AssetContentType::Binary,
            icon_hint: None,
            node_param: None,
            system_dir: PathBuf::from("samples/test/system"),
            user_dir: PathBuf::from("samples/test/user"),
        }
    }

    #[tokio::test]
    async fn process_entry_returns_none_for_directories() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("subdir");
        std::fs::create_dir_all(&dir).unwrap();
        let at = test_asset_type(&["txt"]);
        let perms = RolePermissions::admin();
        assert!(process_entry(dir, false, &perms, &at).await.is_none());
    }

    #[tokio::test]
    async fn process_entry_returns_none_for_disallowed_extension() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("notes.exe");
        std::fs::write(&file, b"x").unwrap();
        let at = test_asset_type(&["txt"]);
        let perms = RolePermissions::admin();
        assert!(process_entry(file, false, &perms, &at).await.is_none());
    }

    #[tokio::test]
    async fn process_entry_returns_none_when_filtered_by_permissions() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("clip.txt");
        std::fs::write(&file, b"x").unwrap();
        let at = test_asset_type(&["txt"]);
        // viewer has no permission to access `samples/test/user/*`.
        let perms = RolePermissions::viewer();
        assert!(process_entry(file, false, &perms, &at).await.is_none());
    }

    #[tokio::test]
    async fn process_entry_returns_user_asset_with_display_name() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("my_cool-file.txt");
        std::fs::write(&file, b"hello").unwrap();
        let at = test_asset_type(&["txt"]);
        let perms = RolePermissions::admin();
        let asset = process_entry(file, false, &perms, &at).await.unwrap();
        assert_eq!(asset.id, "my_cool-file.txt");
        assert_eq!(asset.name, "my cool file");
        assert_eq!(asset.format, "txt");
        assert_eq!(asset.size_bytes, 5);
        assert!(!asset.is_system);
        assert_eq!(asset.type_id, "test");
        assert_eq!(asset.plugin_id, "test-plugin");
        // Path uses the parent of `system_dir` as the base; pinned exactly to
        // catch silent prefix drift in `process_entry`.
        assert_eq!(asset.path, "samples/test/user/my_cool-file.txt");
    }

    #[tokio::test]
    async fn scan_directory_returns_empty_for_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let at = test_asset_type(&["txt"]);
        let perms = RolePermissions::admin();
        let assets =
            scan_directory(&tmp.path().join("does-not-exist"), false, &perms, &at).await.unwrap();
        assert!(assets.is_empty());
    }

    #[tokio::test]
    async fn scan_directory_filters_and_collects() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
        std::fs::write(tmp.path().join("b.txt"), b"bb").unwrap();
        std::fs::write(tmp.path().join("c.exe"), b"x").unwrap();
        std::fs::create_dir_all(tmp.path().join("nested")).unwrap();
        let at = test_asset_type(&["txt"]);
        let perms = RolePermissions::admin();
        let mut assets = scan_directory(tmp.path(), false, &perms, &at).await.unwrap();
        assets.sort_by(|a, b| a.id.cmp(&b.id));
        assert_eq!(assets.len(), 2);
        assert_eq!(assets[0].id, "a.txt");
        assert_eq!(assets[1].id, "b.txt");
    }

    #[tokio::test]
    async fn list_returns_404_for_unknown_type() {
        let state = make_state();
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_returns_sorted_assets_across_scopes() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            65_536,
        );
        std::fs::write(reg.system_dir.join("alpha.slint"), b"// sys").unwrap();
        std::fs::write(reg.user_dir.join("zeta.slint"), b"// user").unwrap();
        std::fs::write(reg.user_dir.join("beta.slint"), b"// user").unwrap();
        install_type(&state, reg).await;

        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder().uri("/api/v1/assets/plugin/slint").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Vec<serde_json::Value> =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        let ids: Vec<&str> = body.iter().map(|v| v["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["alpha.slint", "beta.slint", "zeta.slint"]);
        // sorted by display name (alpha/beta/zeta — underscores replaced).
    }

    #[tokio::test]
    async fn list_returns_empty_when_no_files() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            65_536,
        );
        install_type(&state, reg).await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder().uri("/api/v1/assets/plugin/slint").body(Body::empty()).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert!(body.as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn upload_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let state = make_viewer_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;

        let boundary = "boundary_viewer_upload";
        let body = build_multipart_body(boundary, "file", Some("x.slint"), b"// hi");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn upload_returns_404_for_unknown_type() {
        let state = make_state();
        let boundary = "boundary_unknown_type";
        let body = build_multipart_body(boundary, "file", Some("x.slint"), b"// hi");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(
                Method::POST,
                "/api/v1/assets/plugin/missing",
                boundary,
                body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn upload_rejects_field_without_filename() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;

        let boundary = "boundary_no_filename";
        let body = build_multipart_body(boundary, "file", None, b"data");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(body_string(resp.into_body()).await.contains("No filename"));
    }

    #[tokio::test]
    async fn upload_rejects_disallowed_extension() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let boundary = "boundary_bad_ext";
        let body = build_multipart_body(boundary, "file", Some("bad.exe"), b"x");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn upload_happy_path_writes_file_and_returns_asset() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            4096,
        );
        let user_dir = reg.user_dir.clone();
        install_type(&state, reg).await;

        let boundary = "boundary_happy_upload";
        let payload = b"// hello, world\n";
        let body = build_multipart_body(boundary, "file", Some("hello-world.slint"), payload);
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let v: serde_json::Value =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert_eq!(v["id"], "hello-world.slint");
        assert_eq!(v["name"], "hello world");
        assert_eq!(v["format"], "slint");
        assert_eq!(v["size_bytes"].as_u64().unwrap(), payload.len() as u64);
        assert_eq!(v["is_system"], false);

        let written = std::fs::read(user_dir.join("hello-world.slint")).unwrap();
        assert_eq!(written, payload);
    }

    #[tokio::test]
    async fn upload_returns_409_on_duplicate() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            4096,
        );
        std::fs::write(reg.user_dir.join("dup.slint"), b"existing").unwrap();
        install_type(&state, reg).await;

        let boundary = "boundary_dup_upload";
        let body = build_multipart_body(boundary, "file", Some("dup.slint"), b"new");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn upload_returns_413_when_payload_exceeds_max_size() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            8, // tiny cap
        );
        let user_dir = reg.user_dir.clone();
        install_type(&state, reg).await;

        let boundary = "boundary_too_big";
        let body = build_multipart_body(boundary, "file", Some("big.slint"), b"this is too large");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
        // The partial file should have been cleaned up.
        assert!(!user_dir.join("big.slint").exists());
    }

    #[tokio::test]
    async fn upload_returns_400_when_no_field_present() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;

        let boundary = "boundary_empty_body";
        // Multipart body with closing boundary only — no fields.
        let body = format!("--{boundary}--\r\n").into_bytes();
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(Method::POST, "/api/v1/assets/plugin/slint", boundary, body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let state = make_viewer_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/assets/plugin/slint/a.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_returns_404_for_unknown_type() {
        let state = make_state();
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/assets/plugin/missing/a.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_rejects_path_traversal_in_id() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        // axum decodes %2F -> '/'; check that even after decoding it's rejected.
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/assets/plugin/slint/..%2Fpasswd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_returns_404_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/assets/plugin/slint/missing.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_removes_file_returns_204() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            4096,
        );
        let path = reg.user_dir.join("clip.slint");
        std::fs::write(&path, b"hi").unwrap();
        install_type(&state, reg).await;

        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/api/v1/assets/plugin/slint/clip.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn serve_rejects_path_traversal_in_id() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/slint/file/user/..%2Fpasswd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_rejects_invalid_scope() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/slint/file/wrong-scope/x.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_returns_404_for_unknown_type() {
        let state = make_state();
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/missing/file/user/x.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serve_rejects_disallowed_extension() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/slint/file/user/secret.env")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_returns_text_content_type_for_text_assets() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            4096,
        );
        std::fs::write(reg.user_dir.join("a.slint"), b"// text body").unwrap();
        install_type(&state, reg).await;

        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/slint/file/user/a.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "text/plain; charset=utf-8");
        assert!(resp
            .headers()
            .get(header::CONTENT_DISPOSITION)
            .unwrap()
            .to_str()
            .unwrap()
            .contains("attachment; filename=\"a.slint\""));
        assert_eq!(body_bytes(resp.into_body()).await, b"// text body".to_vec());
    }

    #[tokio::test]
    async fn serve_returns_binary_content_type_for_binary_assets() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "bin",
            "bin-plugin",
            &["bin"],
            AssetContentType::Binary,
            4096,
        );
        std::fs::write(reg.system_dir.join("blob.bin"), b"\x00\x01\x02").unwrap();
        install_type(&state, reg).await;

        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/bin/file/system/blob.bin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_TYPE).unwrap(), "application/octet-stream");
        assert_eq!(body_bytes(resp.into_body()).await, b"\x00\x01\x02".to_vec());
    }

    #[tokio::test]
    async fn serve_returns_404_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/assets/plugin/slint/file/user/missing.slint")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let state = make_viewer_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/user/a.slint")
                    .body(Body::from("// new"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_forbidden_for_system_scope() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/system/a.slint")
                    .body(Body::from("// no"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn update_rejects_traversal_in_id() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/user/..%2Fpasswd")
                    .body(Body::from("oops"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_returns_404_for_unknown_type() {
        let state = make_state();
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/missing/file/user/a.slint")
                    .body(Body::from("body"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_rejects_non_text_asset_types() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "bin",
            "bin-plugin",
            &["bin"],
            AssetContentType::Binary,
            4096,
        );
        std::fs::write(reg.user_dir.join("a.bin"), b"x").unwrap();
        install_type(&state, reg).await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/bin/file/user/a.bin")
                    .body(Body::from("y"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(body_string(resp.into_body()).await.contains("in-place editing"));
    }

    #[tokio::test]
    async fn update_rejects_bad_extension() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/user/bad.exe")
                    .body(Body::from("// hi"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_returns_404_when_target_missing() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/user/missing.slint")
                    .body(Body::from("// hi"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn update_returns_413_when_body_exceeds_max_size() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            4, // tiny cap to trigger the per-type limit (under the 10 MiB router cap)
        );
        std::fs::write(reg.user_dir.join("a.slint"), b"original").unwrap();
        install_type(&state, reg).await;

        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/user/a.slint")
                    .body(Body::from("this is too long"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn update_writes_new_content_and_returns_metadata() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        let reg = registered_type(
            tmp.path(),
            "slint",
            "slint-plugin",
            &["slint"],
            AssetContentType::Text,
            4096,
        );
        let target = reg.user_dir.join("a-file.slint");
        std::fs::write(&target, b"old").unwrap();
        install_type(&state, reg).await;

        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::PUT)
                    .uri("/api/v1/assets/plugin/slint/file/user/a-file.slint")
                    .body(Body::from("// updated"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: serde_json::Value =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert_eq!(v["id"], "a-file.slint");
        assert_eq!(v["name"], "a file");
        assert_eq!(v["format"], "slint");
        assert_eq!(v["size_bytes"].as_u64().unwrap(), 10);
        assert_eq!(std::fs::read(&target).unwrap(), b"// updated");
    }

    #[tokio::test]
    async fn asset_types_lists_core_when_no_plugins_registered() {
        let state = make_state();
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/asset-types").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Vec<serde_json::Value> =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        let ids: std::collections::BTreeSet<&str> =
            v.iter().map(|t| t["type_id"].as_str().unwrap()).collect();
        assert_eq!(
            ids,
            ["audio", "image", "font"].iter().copied().collect::<std::collections::BTreeSet<_>>()
        );
        for entry in &v {
            assert_eq!(entry["source"], "core");
            assert_eq!(entry["editable"], false);
        }
    }

    #[tokio::test]
    async fn asset_types_includes_registered_plugin_types() {
        let tmp = TempDir::new().unwrap();
        let state = make_state();
        install_type(
            &state,
            registered_type(
                tmp.path(),
                "slint",
                "slint-plugin",
                &["slint"],
                AssetContentType::Text,
                4096,
            ),
        )
        .await;
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/asset-types").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Vec<serde_json::Value> =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        let slint =
            v.iter().find(|t| t["type_id"] == "slint").expect("slint type should be registered");
        assert_eq!(slint["source"], "plugin");
        assert_eq!(slint["plugin_id"], "slint-plugin");
        assert_eq!(slint["editable"], true);
        assert_eq!(slint["icon_hint"], "file"); // default when None
    }

    fn minimal_manifest_yaml(id: &str) -> String {
        format!(
            "id: {id}\n\
             version: 0.1.0\n\
             node_kind: plugin::native::{id}\n\
             kind: native\n\
             entrypoint: lib{id}.so\n"
        )
    }

    #[test]
    fn read_local_plugin_manifest_returns_none_when_missing() {
        let tmp = TempDir::new().unwrap();
        let library = tmp.path().join("libsomething.so");
        std::fs::write(&library, b"").unwrap();
        assert!(read_local_plugin_manifest(&library).is_none());
    }

    #[test]
    fn read_local_plugin_manifest_loads_plugin_yml_in_same_dir() {
        let tmp = TempDir::new().unwrap();
        let library = tmp.path().join("libslint.so");
        std::fs::write(&library, b"").unwrap();
        std::fs::write(tmp.path().join("plugin.yml"), minimal_manifest_yaml("slint")).unwrap();
        let manifest = read_local_plugin_manifest(&library).expect("manifest");
        assert_eq!(manifest.id, "slint");
    }

    #[test]
    fn read_local_plugin_manifest_loads_plugin_yaml_in_same_dir() {
        let tmp = TempDir::new().unwrap();
        let library = tmp.path().join("libslint.so");
        std::fs::write(&library, b"").unwrap();
        std::fs::write(tmp.path().join("plugin.yaml"), minimal_manifest_yaml("slint")).unwrap();
        let manifest = read_local_plugin_manifest(&library).expect("manifest");
        assert_eq!(manifest.id, "slint");
    }

    #[test]
    fn read_local_plugin_manifest_loads_stem_plugin_yml_fallback() {
        let tmp = TempDir::new().unwrap();
        let library = tmp.path().join("libslint.so");
        std::fs::write(&library, b"").unwrap();
        std::fs::write(tmp.path().join("slint.plugin.yml"), minimal_manifest_yaml("slint"))
            .unwrap();
        let manifest = read_local_plugin_manifest(&library).expect("manifest");
        assert_eq!(manifest.id, "slint");
    }

    #[test]
    fn read_local_plugin_manifest_returns_none_for_invalid_yaml() {
        let tmp = TempDir::new().unwrap();
        let library = tmp.path().join("libbad.so");
        std::fs::write(&library, b"").unwrap();
        // Missing all required fields — should fail to parse and return None.
        std::fs::write(tmp.path().join("plugin.yml"), "not a manifest").unwrap();
        assert!(read_local_plugin_manifest(&library).is_none());
    }

    /// Exercises the `asset_root.join(relative_system_dir)` path that
    /// `register()` produces.  Previous tests used absolute `system_dir`
    /// values which short-circuit `Path::join`.
    #[tokio::test]
    async fn upload_with_relative_system_dir_uses_asset_root() {
        let tmp = TempDir::new().unwrap();
        let state = make_state_with_asset_root(tmp.path().to_path_buf());

        // Use register() which produces *relative* system_dir/user_dir
        // (e.g. "samples/models/system").
        state
            .plugin_asset_registry
            .register(
                "test-plugin",
                "plugin::native::test-plugin",
                &[PluginAssetSpec {
                    type_id: "models".to_string(),
                    label: "Models".to_string(),
                    extensions: vec!["bin".to_string()],
                    max_size_bytes: 4096,
                    content_type: AssetContentType::Binary,
                    system_dir: None,
                    icon_hint: None,
                    node_param: None,
                }],
            )
            .await;

        // Create the dirs under the asset_root that handlers will resolve.
        let user_dir = tmp.path().join("samples/models/user");
        std::fs::create_dir_all(&user_dir).unwrap();

        let boundary = "----boundary";
        let body = build_multipart_body(boundary, "file", Some("test.bin"), b"model-data");
        let app = plugin_assets_router().with_state(state);
        let resp = app
            .oneshot(multipart_request(
                Method::POST,
                "/api/v1/assets/plugin/models",
                boundary,
                body,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);

        // Verify the file landed under asset_root, not under CWD.
        let uploaded = user_dir.join("test.bin");
        assert!(uploaded.exists(), "file should exist under asset_root");
        assert_eq!(std::fs::read(&uploaded).unwrap(), b"model-data");
    }
}
