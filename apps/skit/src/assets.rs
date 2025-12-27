// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get},
    Json, Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{debug, error, info, warn};

use crate::permissions::Permissions as RolePermissions;
use crate::role_extractor::get_permissions;
use crate::state::AppState;
use streamkit_api::AudioAsset;

// Security limits
const MAX_AUDIO_FILE_SIZE: usize = 100 * 1024 * 1024; // 100MB
const MAX_FILENAME_LENGTH: usize = 255;

// Allowed audio formats
const ALLOWED_AUDIO_FORMATS: &[&str] = &["opus", "ogg", "flac", "mp3", "wav"];

/// Validates a filename for security
fn validate_audio_filename(filename: &str) -> Result<String, AssetsError> {
    // Check length
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(AssetsError::InvalidFilename("Filename too long".to_string()));
    }

    // Check if empty
    if filename.is_empty() {
        return Err(AssetsError::InvalidFilename("Filename cannot be empty".to_string()));
    }

    // Check for path traversal attempts
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(AssetsError::InvalidFilename("Invalid characters in filename".to_string()));
    }

    // Extract extension and validate it's an audio format
    let extension = filename
        .rsplit('.')
        .next()
        .ok_or_else(|| AssetsError::InvalidFilename("File must have an extension".to_string()))?
        .to_lowercase();

    if !ALLOWED_AUDIO_FORMATS.contains(&extension.as_str()) {
        return Err(AssetsError::InvalidFormat(format!(
            "Unsupported audio format: {}. Allowed: {}",
            extension,
            ALLOWED_AUDIO_FORMATS.join(", ")
        )));
    }

    Ok(extension)
}

/// Sanitize filename by removing dangerous characters
fn sanitize_filename(filename: &str) -> String {
    filename
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Parse license file contents
async fn read_license_file(license_path: &PathBuf) -> Option<String> {
    use std::fmt::Write as _;

    fs::read_to_string(license_path).await.map_or(None, |contents| {
        // Extract relevant info from SPDX license file
        let mut license_info = String::new();
        // REUSE-IgnoreStart
        for line in contents.lines() {
            if line.starts_with("SPDX-License-Identifier:") {
                if let Some(id) = line.split(':').nth(1) {
                    let _ = writeln!(license_info, "License: {}", id.trim());
                }
            }
            if line.starts_with("SPDX-FileCopyrightText:") {
                if let Some(copyright) = line.split(':').nth(1) {
                    let _ = write!(license_info, "Copyright: {}", copyright.trim());
                }
            }
        }
        // REUSE-IgnoreEnd
        if license_info.is_empty() {
            None
        } else {
            Some(license_info.trim().to_string())
        }
    })
}

/// Process a single directory entry and convert it to an AudioAsset if valid
/// Returns None if the entry should be skipped
async fn process_audio_entry(
    path: std::path::PathBuf,
    is_system: bool,
    perms: &RolePermissions,
) -> Option<AudioAsset> {
    // Skip directories and license files
    if path.is_dir() || path.extension().and_then(|s| s.to_str()) == Some("license") {
        return None;
    }

    let filename = path.file_name().and_then(|s| s.to_str())?.to_string();

    // Validate extension
    let extension = path.extension().and_then(|s| s.to_str()).map(str::to_lowercase)?;

    if !ALLOWED_AUDIO_FORMATS.contains(&extension.as_str()) {
        return None;
    }

    // Get file metadata
    let metadata = fs::metadata(&path).await.ok()?;
    let size_bytes = metadata.len();

    // Generate ID from full filename (including extension) to ensure uniqueness
    let id = filename.clone();

    // Generate display name from filename without extension
    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");

    // Check permissions
    let asset_path_str = if is_system {
        format!("samples/audio/system/{filename}")
    } else {
        format!("samples/audio/user/{filename}")
    };

    if !perms.is_asset_allowed(&asset_path_str) {
        debug!("Asset filtered by permissions: {}", asset_path_str);
        return None;
    }

    // Read license file if it exists
    let license_path = path.with_extension(format!("{extension}.license"));
    let license = read_license_file(&license_path).await;

    Some(AudioAsset {
        id,
        name: display_name,
        path: asset_path_str,
        format: extension,
        size_bytes,
        license,
        is_system,
    })
}

/// Scan a directory for audio assets
async fn scan_audio_directory(
    dir_path: &PathBuf,
    is_system: bool,
    perms: &RolePermissions,
) -> Result<Vec<AudioAsset>, AssetsError> {
    let mut assets = Vec::new();

    // Check if directory exists
    if !dir_path.exists() {
        warn!("Audio directory does not exist: {:?}", dir_path);
        return Ok(assets);
    }

    let mut entries = fs::read_dir(dir_path)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to read directory: {e}")))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to read entry: {e}")))?
    {
        if let Some(asset) = process_audio_entry(entry.path(), is_system, perms).await {
            assets.push(asset);
        }
    }

    Ok(assets)
}

/// List all audio assets (system + user) with permission filtering
pub async fn list_assets_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    match list_assets(&app_state, &perms).await {
        Ok(assets) => {
            info!("Listed {} audio assets", assets.len());
            Json(assets).into_response()
        },
        Err(e) => {
            error!("Failed to list assets: {}", e);
            e.into_response()
        },
    }
}

async fn list_assets(
    _app_state: &AppState,
    perms: &RolePermissions,
) -> Result<Vec<AudioAsset>, AssetsError> {
    // Audio assets are in samples/audio/, not samples/pipelines/
    let base_path = PathBuf::from("samples/audio");
    let system_path = base_path.join("system");
    let user_path = base_path.join("user");

    let mut all_assets = Vec::new();

    // Scan system assets
    let system_assets = scan_audio_directory(&system_path, true, perms).await?;
    all_assets.extend(system_assets);

    // Scan user assets
    let user_assets = scan_audio_directory(&user_path, false, perms).await?;
    all_assets.extend(user_assets);

    // Sort by name for consistent ordering
    all_assets.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(all_assets)
}

/// Stream an uploaded multipart field to disk with size enforcement.
async fn write_upload_stream_to_disk(
    mut field: axum::extract::multipart::Field<'_>,
    file_path: &std::path::Path,
    extension: &str,
) -> Result<usize, AssetsError> {
    use tokio::fs::OpenOptions;

    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(file_path)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to create file: {e}")))?;

    let mut total_bytes: usize = 0;
    loop {
        match field.chunk().await {
            Ok(Some(chunk)) => {
                total_bytes = total_bytes.saturating_add(chunk.len());
                if total_bytes > MAX_AUDIO_FILE_SIZE {
                    let _ = fs::remove_file(file_path).await;
                    return Err(AssetsError::FileTooLarge(MAX_AUDIO_FILE_SIZE));
                }

                if let Err(e) = file.write_all(&chunk).await {
                    let _ = fs::remove_file(file_path).await;
                    return Err(AssetsError::IoError(format!("Failed to write file: {e}")));
                }
            },
            Ok(None) => break,
            Err(e) => {
                let _ = fs::remove_file(file_path).await;
                return Err(AssetsError::InvalidRequest(format!(
                    "Failed to read upload stream: {e}"
                )));
            },
        }
    }

    // Create default license file (best-effort).
    let license_path = file_path.with_extension(format!("{extension}.license"));
    // REUSE-IgnoreStart
    let default_license =
        "SPDX-FileCopyrightText: © 2025 User Upload\n\nSPDX-License-Identifier: CC0-1.0\n";
    // REUSE-IgnoreEnd
    if let Err(e) = fs::write(&license_path, default_license).await {
        warn!("Failed to create license file: {}", e);
    }

    Ok(total_bytes)
}

/// Build AudioAsset response for uploaded file
fn build_upload_response(
    filename: &str,
    extension: &str,
    _file_path: &std::path::Path,
    data_len: usize,
) -> AudioAsset {
    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");

    let relative_path = format!("samples/audio/user/{filename}");

    AudioAsset {
        id: filename.to_string(),
        name: display_name,
        path: relative_path,
        format: extension.to_string(),
        size_bytes: data_len as u64,
        license: Some("License: CC0-1.0\nCopyright: © 2025 User Upload".to_string()),
        is_system: false,
    }
}

/// Core upload logic after permission check
async fn process_upload(
    filename: String,
    extension: String,
    field: axum::extract::multipart::Field<'_>,
) -> Result<AudioAsset, AssetsError> {
    let base_path = PathBuf::from("samples/audio");
    let user_dir = base_path.join("user");

    fs::create_dir_all(&user_dir)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to create directory: {e}")))?;

    let file_path = user_dir.join(&filename);

    if file_path.exists() {
        return Err(AssetsError::FileExists(filename));
    }

    let written_bytes = write_upload_stream_to_disk(field, &file_path, &extension).await?;

    info!("Uploaded audio asset: {}", filename);

    Ok(build_upload_response(&filename, &extension, &file_path, written_bytes))
}

/// Upload a new audio asset (user directory only)
pub async fn upload_asset_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    if !perms.upload_assets {
        return AssetsError::Forbidden.into_response();
    }

    let field = match multipart.next_field().await {
        Ok(Some(field)) => field,
        Ok(None) => {
            return AssetsError::InvalidRequest("No file provided".to_string()).into_response()
        },
        Err(e) => {
            return AssetsError::InvalidRequest(format!("Failed to read multipart: {e}"))
                .into_response()
        },
    };

    let filename = match field.file_name() {
        Some(name) => sanitize_filename(name),
        None => {
            return AssetsError::InvalidRequest("No filename provided".to_string()).into_response()
        },
    };
    let extension = match validate_audio_filename(&filename) {
        Ok(ext) => ext,
        Err(e) => return e.into_response(),
    };

    match process_upload(filename, extension, field).await {
        Ok(asset) => Json(asset).into_response(),
        Err(e) => {
            error!("Failed to process upload: {}", e);
            e.into_response()
        },
    }
}

/// Validate that a file path is within the user directory (security check)
fn validate_file_in_user_directory(
    file_path: &std::path::Path,
    user_dir: &std::path::Path,
) -> Result<(), AssetsError> {
    let canonical = file_path
        .canonicalize()
        .map_err(|e| AssetsError::IoError(format!("Failed to resolve file path: {e}")))?;

    let canonical_user_dir = user_dir
        .canonicalize()
        .map_err(|_| AssetsError::IoError("Failed to resolve user directory".to_string()))?;

    if !canonical.starts_with(&canonical_user_dir) {
        error!("Attempt to delete non-user asset: {:?}", canonical);
        return Err(AssetsError::Forbidden);
    }

    Ok(())
}

/// Delete audio file and its associated license file
async fn delete_audio_files(
    file_path: &std::path::Path,
    extension: &str,
) -> Result<(), AssetsError> {
    fs::remove_file(file_path)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to delete file: {e}")))?;

    // Delete license file if it exists
    let license_path = file_path.with_extension(format!("{extension}.license"));
    if license_path.exists() {
        if let Err(e) = fs::remove_file(&license_path).await {
            warn!("Failed to delete license file: {}", e);
        }
    }

    Ok(())
}

/// Delete an audio asset (user directory only)
pub async fn delete_asset_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    if !perms.delete_assets {
        return AssetsError::Forbidden.into_response();
    }

    let base_path = PathBuf::from("samples/audio");
    let user_dir = base_path.join("user");
    let file_path = user_dir.join(&id);

    // Extract extension from filename
    let extension = match id.rsplit('.').next() {
        Some(ext) => ext.to_string(),
        None => return AssetsError::NotFound(id).into_response(),
    };

    if !file_path.exists() {
        return AssetsError::NotFound(id).into_response();
    }

    if let Err(e) = validate_file_in_user_directory(&file_path, &user_dir) {
        return e.into_response();
    }

    if let Err(e) = delete_audio_files(&file_path, &extension).await {
        error!("Failed to delete audio file: {}", e);
        return e.into_response();
    }

    info!("Deleted audio asset: {}", id);
    StatusCode::NO_CONTENT.into_response()
}

/// Create router for audio asset endpoints
pub fn assets_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/assets/audio",
            get(list_assets_handler)
                .post(upload_asset_handler)
                .layer(DefaultBodyLimit::max(MAX_AUDIO_FILE_SIZE)),
        )
        .route("/api/v1/assets/audio/{id}", delete(delete_asset_handler))
}

// Error types

#[derive(Debug)]
pub enum AssetsError {
    IoError(String),
    InvalidFilename(String),
    InvalidFormat(String),
    InvalidRequest(String),
    FileTooLarge(usize),
    FileExists(String),
    NotFound(String),
    Forbidden,
}

impl IntoResponse for AssetsError {
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

impl std::fmt::Display for AssetsError {
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

impl std::error::Error for AssetsError {}
