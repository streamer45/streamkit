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
use streamkit_api::FontAsset;
use streamkit_api::ImageAsset;

// Security limits
const MAX_AUDIO_FILE_SIZE: usize = 100 * 1024 * 1024; // 100MB
const MAX_FILENAME_LENGTH: usize = 255;

// Allowed audio formats
const ALLOWED_AUDIO_FORMATS: &[&str] = &["opus", "ogg", "flac", "mp3", "wav"];

/// Validates a filename for security
fn validate_audio_filename(filename: &str) -> Result<String, AssetsError> {
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(AssetsError::InvalidFilename("Filename too long".to_string()));
    }

    if filename.is_empty() {
        return Err(AssetsError::InvalidFilename("Filename cannot be empty".to_string()));
    }

    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(AssetsError::InvalidFilename("Invalid characters in filename".to_string()));
    }

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
        .map(
            |c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' },
        )
        .collect()
}

/// Parse license file contents
async fn read_license_file(license_path: &PathBuf) -> Option<String> {
    use std::fmt::Write as _;

    fs::read_to_string(license_path).await.map_or(None, |contents| {
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

    let extension = path.extension().and_then(|s| s.to_str()).map(str::to_lowercase)?;

    if !ALLOWED_AUDIO_FORMATS.contains(&extension.as_str()) {
        return None;
    }

    let metadata = fs::metadata(&path).await.ok()?;
    let size_bytes = metadata.len();

    let id = filename.clone();

    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");

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
    app_state: &AppState,
    perms: &RolePermissions,
) -> Result<Vec<AudioAsset>, AssetsError> {
    let base_path = app_state.asset_root.join("samples/audio");
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
///
/// On any error the partially-written file is removed before returning.
/// Callers that need a REUSE license sidecar should create it after this
/// function succeeds.
async fn stream_field_to_file(
    mut field: axum::extract::multipart::Field<'_>,
    file_path: &std::path::Path,
    max_size: usize,
) -> Result<usize, AssetsError> {
    use tokio::fs::OpenOptions;

    let open_result = OpenOptions::new().create_new(true).write(true).open(file_path).await;

    let mut file = match open_result {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(AssetsError::FileExists(
                file_path.file_name().and_then(|n| n.to_str()).unwrap_or("unknown").to_string(),
            ));
        },
        Err(e) => return Err(AssetsError::IoError(format!("Failed to create file: {e}"))),
    };

    // Inner block: any error triggers a single cleanup path below.
    let result = async {
        let mut total_bytes: usize = 0;
        loop {
            match field.chunk().await {
                Ok(Some(chunk)) => {
                    total_bytes = total_bytes.saturating_add(chunk.len());
                    if total_bytes > max_size {
                        return Err(AssetsError::FileTooLarge(max_size));
                    }
                    file.write_all(&chunk)
                        .await
                        .map_err(|e| AssetsError::IoError(format!("Failed to write file: {e}")))?;
                },
                Ok(None) => break,
                Err(e) => {
                    return Err(AssetsError::InvalidRequest(format!(
                        "Failed to read upload stream: {e}"
                    )));
                },
            }
        }

        // Flush pending writes — tokio::fs::File::write_all returns as soon as
        // data is copied to an internal buffer and a blocking write is spawned,
        // so the last write may still be in-flight when the File is dropped.
        file.flush()
            .await
            .map_err(|e| AssetsError::IoError(format!("Failed to flush file: {e}")))?;

        Ok(total_bytes)
    }
    .await;

    if result.is_err() {
        let _ = fs::remove_file(file_path).await;
    }

    result
}

/// Create a default REUSE license sidecar next to the uploaded file.
fn create_license_sidecar(
    file_path: &std::path::Path,
    extension: &str,
) -> impl std::future::Future<Output = ()> + Send + 'static {
    let license_path = file_path.with_extension(format!("{extension}.license"));
    async move {
        // REUSE-IgnoreStart
        let default_license =
            "SPDX-FileCopyrightText: © 2025 User Upload\n\nSPDX-License-Identifier: CC0-1.0\n";
        // REUSE-IgnoreEnd
        if let Err(e) = fs::write(&license_path, default_license).await {
            warn!("Failed to create license file: {}", e);
        }
    }
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
    asset_root: &std::path::Path,
) -> Result<AudioAsset, AssetsError> {
    let base_path = asset_root.join("samples/audio");
    let user_dir = base_path.join("user");

    fs::create_dir_all(&user_dir)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to create directory: {e}")))?;

    let file_path = user_dir.join(&filename);

    if file_path.exists() {
        return Err(AssetsError::FileExists(filename));
    }

    let written_bytes = stream_field_to_file(field, &file_path, MAX_AUDIO_FILE_SIZE).await?;
    create_license_sidecar(&file_path, &extension).await;

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

    match process_upload(filename, extension, field, &app_state.asset_root).await {
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

    let base_path = app_state.asset_root.join("samples/audio");
    let user_dir = base_path.join("user");
    let file_path = user_dir.join(&id);

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

// Security limits for image assets
const MAX_IMAGE_FILE_SIZE: usize = 10 * 1024 * 1024; // 10MB
const MAX_IMAGE_PIXELS: u64 = 40_000_000; // ~40 MP — bounds decoded RGBA to ~160 MB

// Allowed image formats
const ALLOWED_IMAGE_FORMATS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "svg", "svgz"];

/// Validates a filename for image asset security
fn validate_image_filename(filename: &str) -> Result<String, AssetsError> {
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(AssetsError::InvalidFilename("Filename too long".to_string()));
    }

    if filename.is_empty() {
        return Err(AssetsError::InvalidFilename("Filename cannot be empty".to_string()));
    }

    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(AssetsError::InvalidFilename("Invalid characters in filename".to_string()));
    }

    // Require at least one '.' so the rsplit produces a real extension.
    let extension = match filename.rsplit('.').next() {
        Some(ext) if filename.contains('.') => ext.to_lowercase(),
        _ => return Err(AssetsError::InvalidFilename("File must have an extension".to_string())),
    };

    if !ALLOWED_IMAGE_FORMATS.contains(&extension.as_str()) {
        return Err(AssetsError::InvalidFormat(format!(
            "Unsupported image format: {}. Allowed: {}",
            extension,
            ALLOWED_IMAGE_FORMATS.join(", ")
        )));
    }

    Ok(extension)
}

/// Process a single directory entry and convert it to an ImageAsset if valid
async fn process_image_entry(
    path: std::path::PathBuf,
    is_system: bool,
    perms: &RolePermissions,
) -> Option<ImageAsset> {
    if path.is_dir() || path.extension().and_then(|s| s.to_str()) == Some("license") {
        return None;
    }

    let filename = path.file_name().and_then(|s| s.to_str())?.to_string();

    let extension = path.extension().and_then(|s| s.to_str()).map(str::to_lowercase)?;

    if !ALLOWED_IMAGE_FORMATS.contains(&extension.as_str()) {
        return None;
    }

    let metadata = fs::metadata(&path).await.ok()?;
    let size_bytes = metadata.len();

    let id = filename.clone();

    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");

    let asset_path_str = if is_system {
        format!("samples/images/system/{filename}")
    } else {
        format!("samples/images/user/{filename}")
    };

    if !perms.is_asset_allowed(&asset_path_str) {
        debug!("Image asset filtered by permissions: {}", asset_path_str);
        return None;
    }

    // Read only the image header to extract dimensions (avoids full pixel decode).
    // SVGs use the resvg parser; raster formats use ImageReader::open().
    let (width, height) = if extension == "svg" || extension == "svgz" {
        let svg_data = fs::read(&path).await.ok()?;
        // SVG parsing is CPU-intensive; run off the async runtime.
        tokio::task::spawn_blocking(move || {
            streamkit_nodes::video::compositor::overlay::svg_viewbox_dimensions(&svg_data)
        })
        .await
        .ok()??
    } else {
        // Use ImageReader::open() which reads directly from file rather than
        // loading the entire file into memory first.
        let path_clone = path.clone();
        match tokio::task::spawn_blocking(move || {
            image::ImageReader::open(&path_clone)
                .and_then(image::ImageReader::with_guessed_format)
                .map_err(|e| e.to_string())
                .and_then(|r| r.into_dimensions().map_err(|e| e.to_string()))
        })
        .await
        {
            Ok(Ok(dims)) => dims,
            Ok(Err(e)) => {
                warn!("Failed to read image dimensions {}: {}", filename, e);
                return None;
            },
            Err(e) => {
                warn!("Failed to read image dimensions {}: {}", filename, e);
                return None;
            },
        }
    };

    Some(ImageAsset {
        id,
        name: display_name,
        path: asset_path_str,
        format: extension,
        width,
        height,
        size_bytes,
        is_system,
    })
}

/// Scan a directory for image assets
async fn scan_image_directory(
    dir_path: &PathBuf,
    is_system: bool,
    perms: &RolePermissions,
) -> Result<Vec<ImageAsset>, AssetsError> {
    let mut assets = Vec::new();

    if !dir_path.exists() {
        warn!("Image directory does not exist: {:?}", dir_path);
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
        if let Some(asset) = process_image_entry(entry.path(), is_system, perms).await {
            assets.push(asset);
        }
    }

    Ok(assets)
}

/// List all image assets (system + user) with permission filtering
pub async fn list_image_assets_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    match list_image_assets(&app_state.asset_root, &perms).await {
        Ok(assets) => {
            info!("Listed {} image assets", assets.len());
            Json(assets).into_response()
        },
        Err(e) => {
            error!("Failed to list image assets: {}", e);
            e.into_response()
        },
    }
}

async fn list_image_assets(
    asset_root: &std::path::Path,
    perms: &RolePermissions,
) -> Result<Vec<ImageAsset>, AssetsError> {
    let base_path = asset_root.join("samples/images");
    let system_path = base_path.join("system");
    let user_path = base_path.join("user");

    let mut all_assets = Vec::new();

    let system_assets = scan_image_directory(&system_path, true, perms).await?;
    all_assets.extend(system_assets);

    let user_assets = scan_image_directory(&user_path, false, perms).await?;
    all_assets.extend(user_assets);

    all_assets.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(all_assets)
}

/// Core image upload logic after permission check
async fn process_image_upload(
    filename: String,
    extension: String,
    field: axum::extract::multipart::Field<'_>,
    max_image_dimension: u32,
    asset_root: &std::path::Path,
) -> Result<ImageAsset, AssetsError> {
    let base_path = asset_root.join("samples/images");
    let user_dir = base_path.join("user");

    fs::create_dir_all(&user_dir)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to create directory: {e}")))?;

    let file_path = user_dir.join(&filename);

    let written_bytes = stream_field_to_file(field, &file_path, MAX_IMAGE_FILE_SIZE).await?;

    // SVG validation: parse with resvg to check validity and extract dimensions.
    // Skip raster decode path entirely for SVGs.
    if extension == "svg" || extension == "svgz" {
        let file_data = match fs::read(&file_path).await {
            Ok(data) => data,
            Err(e) => {
                let _ = fs::remove_file(&file_path).await;
                return Err(AssetsError::IoError(format!("Failed to read uploaded file: {e}")));
            },
        };

        // SVG parsing (Tree::from_data) is CPU-intensive; run off the async runtime.
        let dims = tokio::task::spawn_blocking(move || {
            streamkit_nodes::video::compositor::overlay::svg_viewbox_dimensions(&file_data)
        })
        .await;
        let (width, height) = match dims {
            Ok(Some((w, h))) => {
                if w > max_image_dimension || h > max_image_dimension {
                    let _ = fs::remove_file(&file_path).await;
                    return Err(AssetsError::InvalidFormat(format!(
                        "SVG dimensions {w}x{h} exceed maximum \
                         {max_image_dimension}x{max_image_dimension}"
                    )));
                }
                (w, h)
            },
            Ok(None) => {
                let _ = fs::remove_file(&file_path).await;
                return Err(AssetsError::InvalidFormat(
                    "Uploaded file is not a valid SVG".to_string(),
                ));
            },
            Err(e) => {
                let _ = fs::remove_file(&file_path).await;
                return Err(AssetsError::IoError(format!("SVG validation task failed: {e}")));
            },
        };

        let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
        let display_name = name_without_ext.replace(['_', '-'], " ");
        let relative_path = format!("samples/images/user/{filename}");

        info!("Uploaded SVG image asset: {} ({}x{})", filename, width, height);

        return Ok(ImageAsset {
            id: filename,
            name: display_name,
            path: relative_path,
            format: extension,
            width,
            height,
            size_bytes: written_bytes as u64,
            is_system: false,
        });
    }

    // Read the file once — used for both header-only dimension check and full decode.
    let file_data = match fs::read(&file_path).await {
        Ok(data) => data,
        Err(e) => {
            let _ = fs::remove_file(&file_path).await;
            return Err(AssetsError::IoError(format!("Failed to read uploaded file: {e}")));
        },
    };

    // Header-only dimension check to catch decompression bombs early
    // (e.g. a tiny PNG that decompresses to a 50000×50000 pixel buffer),
    // then full decode to validate the entire image is well-formed.
    let decode_result = tokio::task::spawn_blocking(move || {
        use image::GenericImageView;

        // Header-only pass — cheap dimension extraction without full decode.
        let header_dims = image::ImageReader::new(std::io::Cursor::new(&file_data))
            .with_guessed_format()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_dimensions().map_err(|e| e.to_string()));

        match header_dims {
            Ok((w, h)) if w > max_image_dimension || h > max_image_dimension => {
                return Err(format!(
                    "Image dimensions {w}x{h} exceed maximum \
                     {max_image_dimension}x{max_image_dimension}"
                ));
            },
            Ok((w, h)) if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS => {
                return Err(format!(
                    "Image pixel count {}x{} = {} exceeds maximum {MAX_IMAGE_PIXELS}",
                    w,
                    h,
                    u64::from(w) * u64::from(h),
                ));
            },
            Err(e) => return Err(format!("Uploaded file is not a valid image: {e}")),
            _ => {},
        }

        // Full decode — validates the entire image is well-formed, not just the header.
        image::load_from_memory(&file_data)
            .map(|img| img.dimensions())
            .map_err(|e| format!("Uploaded file is not a valid image: {e}"))
    })
    .await;
    let (width, height) = match decode_result {
        Ok(Ok(dims)) => dims,
        Ok(Err(e)) => {
            let _ = fs::remove_file(&file_path).await;
            return Err(AssetsError::InvalidFormat(e));
        },
        Err(e) => {
            let _ = fs::remove_file(&file_path).await;
            return Err(AssetsError::IoError(format!("Image decode task failed: {e}")));
        },
    };

    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");
    let relative_path = format!("samples/images/user/{filename}");

    info!("Uploaded image asset: {} ({}x{})", filename, width, height);

    Ok(ImageAsset {
        id: filename,
        name: display_name,
        path: relative_path,
        format: extension,
        width,
        height,
        size_bytes: written_bytes as u64,
        is_system: false,
    })
}

/// Upload a new image asset (user directory only)
pub async fn upload_image_asset_handler(
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
    let extension = match validate_image_filename(&filename) {
        Ok(ext) => ext,
        Err(e) => return e.into_response(),
    };

    let max_dim = app_state.config.compositor.max_image_dimension;
    match process_image_upload(filename, extension, field, max_dim, &app_state.asset_root).await {
        Ok(asset) => Json(asset).into_response(),
        Err(e) => {
            error!("Failed to process image upload: {}", e);
            e.into_response()
        },
    }
}

/// Delete an image asset (user directory only)
pub async fn delete_image_asset_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    if !perms.delete_assets {
        return AssetsError::Forbidden.into_response();
    }

    let base_path = app_state.asset_root.join("samples/images");
    let user_dir = base_path.join("user");
    let file_path = user_dir.join(&id);

    if !file_path.exists() {
        return AssetsError::NotFound(id).into_response();
    }

    if let Err(e) = validate_file_in_user_directory(&file_path, &user_dir) {
        return e.into_response();
    }

    if let Err(e) = fs::remove_file(&file_path)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to delete file: {e}")))
    {
        error!("Failed to delete image file: {}", e);
        return e.into_response();
    }

    info!("Deleted image asset: {}", id);
    StatusCode::NO_CONTENT.into_response()
}

/// Serve an image asset file by ID (filename).
///
/// Returns the raw image bytes with an appropriate `Content-Type` header so
/// the browser can render it directly (e.g. in an `<img>` tag or on a canvas).
/// Looks in both the user and system directories.
async fn serve_image_asset_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((scope, id)): Path<(String, String)>,
) -> impl IntoResponse {
    use axum::http::header;

    let perms = get_permissions(&headers, &app_state);

    // Basic filename validation to prevent path traversal
    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return AssetsError::InvalidFilename("Invalid characters in filename".to_string())
            .into_response();
    }

    if scope != "user" && scope != "system" {
        return AssetsError::InvalidFilename(
            "Invalid scope: must be 'user' or 'system'".to_string(),
        )
        .into_response();
    }

    let file_path = app_state.asset_root.join("samples/images").join(&scope).join(&id);
    let asset_path_str = format!("samples/images/{scope}/{id}");

    // Reject files without an allowed image extension
    let extension = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    if !ALLOWED_IMAGE_FORMATS.contains(&extension.as_str()) {
        return AssetsError::InvalidFormat(format!("Not an allowed image format: {extension}"))
            .into_response();
    }

    if !perms.is_asset_allowed(&asset_path_str) {
        return AssetsError::Forbidden.into_response();
    }

    if !file_path.exists() {
        return AssetsError::NotFound(id).into_response();
    }

    let extension = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();

    let content_type = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" | "svgz" => "image/svg+xml",
        _ => "application/octet-stream",
    };

    // SVGs can contain <script> and event handlers; restrict execution via
    // CSP to prevent stored XSS when a user-uploaded SVG is opened in a browser tab.
    let is_svg = extension == "svg";
    let is_svgz = extension == "svgz";

    match fs::read(&file_path).await {
        Ok(data) => {
            if is_svg || is_svgz {
                let mut response = axum::http::Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::CACHE_CONTROL, "public, must-revalidate")
                    .header(header::CONTENT_DISPOSITION, "inline")
                    .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
                    .header(
                        header::CONTENT_SECURITY_POLICY,
                        "default-src 'none'; style-src 'unsafe-inline'",
                    );
                if is_svgz {
                    response = response.header(header::CONTENT_ENCODING, "gzip");
                }
                #[allow(clippy::expect_used)]
                // Builder only fails if status/headers are invalid; ours are all static.
                response.body(axum::body::Body::from(data)).expect("valid response").into_response()
            } else {
                (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, content_type.to_string()),
                        (header::CACHE_CONTROL, "public, must-revalidate".to_string()),
                    ],
                    data,
                )
                    .into_response()
            }
        },
        Err(e) => {
            error!("Failed to read image file {:?}: {}", file_path, e);
            AssetsError::IoError(format!("Failed to read file: {e}")).into_response()
        },
    }
}

/// Create router for image asset endpoints
pub fn image_assets_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/assets/images",
            get(list_image_assets_handler)
                .post(upload_image_asset_handler)
                .layer(DefaultBodyLimit::max(MAX_IMAGE_FILE_SIZE)),
        )
        .route("/api/v1/assets/images/file/{scope}/{id}", get(serve_image_asset_handler))
        .route("/api/v1/assets/images/{id}", delete(delete_image_asset_handler))
}

// Security limits for font assets
const MAX_FONT_FILE_SIZE: usize = 10 * 1024 * 1024; // 10MB

// Allowed font formats
const ALLOWED_FONT_FORMATS: &[&str] = &["ttf", "otf"];

/// Validates a filename for font asset security
fn validate_font_filename(filename: &str) -> Result<String, AssetsError> {
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(AssetsError::InvalidFilename("Filename too long".to_string()));
    }

    if filename.is_empty() {
        return Err(AssetsError::InvalidFilename("Filename cannot be empty".to_string()));
    }

    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(AssetsError::InvalidFilename("Invalid characters in filename".to_string()));
    }

    let extension = match filename.rsplit('.').next() {
        Some(ext) if filename.contains('.') => ext.to_lowercase(),
        _ => return Err(AssetsError::InvalidFilename("File must have an extension".to_string())),
    };

    if !ALLOWED_FONT_FORMATS.contains(&extension.as_str()) {
        return Err(AssetsError::InvalidFormat(format!(
            "Unsupported font format: {}. Allowed: {}",
            extension,
            ALLOWED_FONT_FORMATS.join(", ")
        )));
    }

    Ok(extension)
}

/// Process a single directory entry and convert it to a FontAsset if valid
async fn process_font_entry(
    path: std::path::PathBuf,
    is_system: bool,
    perms: &RolePermissions,
) -> Option<FontAsset> {
    if path.is_dir() || path.extension().and_then(|s| s.to_str()) == Some("license") {
        return None;
    }

    let filename = path.file_name().and_then(|s| s.to_str())?.to_string();

    let extension = path.extension().and_then(|s| s.to_str()).map(str::to_lowercase)?;

    if !ALLOWED_FONT_FORMATS.contains(&extension.as_str()) {
        return None;
    }

    let metadata = fs::metadata(&path).await.ok()?;
    let size_bytes = metadata.len();

    let id = filename.clone();

    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");

    let asset_path_str = if is_system {
        format!("samples/fonts/system/{filename}")
    } else {
        format!("samples/fonts/user/{filename}")
    };

    if !perms.is_asset_allowed(&asset_path_str) {
        debug!("Font asset filtered by permissions: {}", asset_path_str);
        return None;
    }

    Some(FontAsset {
        id,
        name: display_name,
        path: asset_path_str,
        format: extension,
        size_bytes,
        is_system,
    })
}

/// Scan a directory for font assets
async fn scan_font_directory(
    dir_path: &PathBuf,
    is_system: bool,
    perms: &RolePermissions,
) -> Result<Vec<FontAsset>, AssetsError> {
    let mut assets = Vec::new();

    if !dir_path.exists() {
        warn!("Font directory does not exist: {:?}", dir_path);
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
        if let Some(asset) = process_font_entry(entry.path(), is_system, perms).await {
            assets.push(asset);
        }
    }

    Ok(assets)
}

/// List all font assets (system + user) with permission filtering
pub async fn list_font_assets_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    match list_font_assets(&app_state.asset_root, &perms).await {
        Ok(assets) => {
            info!("Listed {} font assets", assets.len());
            Json(assets).into_response()
        },
        Err(e) => {
            error!("Failed to list font assets: {}", e);
            e.into_response()
        },
    }
}

async fn list_font_assets(
    asset_root: &std::path::Path,
    perms: &RolePermissions,
) -> Result<Vec<FontAsset>, AssetsError> {
    let base_path = asset_root.join("samples/fonts");
    let system_path = base_path.join("system");
    let user_path = base_path.join("user");

    let mut all_assets = Vec::new();

    let system_assets = scan_font_directory(&system_path, true, perms).await?;
    all_assets.extend(system_assets);

    let user_assets = scan_font_directory(&user_path, false, perms).await?;
    all_assets.extend(user_assets);

    all_assets.sort_by(|a, b| a.name.cmp(&b.name));

    Ok(all_assets)
}

/// Core font upload logic after permission check
async fn process_font_upload(
    filename: String,
    extension: String,
    field: axum::extract::multipart::Field<'_>,
    asset_root: &std::path::Path,
) -> Result<FontAsset, AssetsError> {
    let base_path = asset_root.join("samples/fonts");
    let user_dir = base_path.join("user");

    fs::create_dir_all(&user_dir)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to create directory: {e}")))?;

    let file_path = user_dir.join(&filename);

    let written_bytes = stream_field_to_file(field, &file_path, MAX_FONT_FILE_SIZE).await?;
    create_license_sidecar(&file_path, &extension).await;

    let header = match fs::read(&file_path).await {
        Ok(data) if data.len() >= 4 => data[..4].to_vec(),
        Ok(_) => {
            let _ = fs::remove_file(&file_path).await;
            return Err(AssetsError::InvalidFormat("File too small to be a font".to_string()));
        },
        Err(e) => {
            let _ = fs::remove_file(&file_path).await;
            return Err(AssetsError::IoError(format!("Failed to read uploaded file: {e}")));
        },
    };

    // TTF: starts with 0x00010000 or 'true' (0x74727565)
    // OTF/CFF: starts with 'OTTO' (0x4F54544F)
    // TTC: starts with 'ttcf' (0x74746366)
    let is_valid_font = header == [0x00, 0x01, 0x00, 0x00]
        || header == [0x4F, 0x54, 0x54, 0x4F] // OTTO
        || header == [0x74, 0x72, 0x75, 0x65] // true
        || header == [0x74, 0x74, 0x63, 0x66]; // ttcf

    if !is_valid_font {
        let _ = fs::remove_file(&file_path).await;
        // Also remove the license sidecar created by create_license_sidecar.
        let license_path = file_path.with_extension(format!("{extension}.license"));
        let _ = fs::remove_file(&license_path).await;
        return Err(AssetsError::InvalidFormat(
            "Uploaded file is not a valid TTF/OTF font (invalid magic bytes)".to_string(),
        ));
    }

    let name_without_ext = filename.trim_end_matches(&format!(".{extension}"));
    let display_name = name_without_ext.replace(['_', '-'], " ");
    let relative_path = format!("samples/fonts/user/{filename}");

    info!("Uploaded font asset: {}", filename);

    Ok(FontAsset {
        id: filename,
        name: display_name,
        path: relative_path,
        format: extension,
        size_bytes: written_bytes as u64,
        is_system: false,
    })
}

/// Upload a new font asset (user directory only)
pub async fn upload_font_asset_handler(
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
    let extension = match validate_font_filename(&filename) {
        Ok(ext) => ext,
        Err(e) => return e.into_response(),
    };

    match process_font_upload(filename, extension, field, &app_state.asset_root).await {
        Ok(asset) => Json(asset).into_response(),
        Err(e) => {
            error!("Failed to process font upload: {}", e);
            e.into_response()
        },
    }
}

/// Delete a font asset (user directory only)
pub async fn delete_font_asset_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);

    if !perms.delete_assets {
        return AssetsError::Forbidden.into_response();
    }

    let base_path = app_state.asset_root.join("samples/fonts");
    let user_dir = base_path.join("user");
    let file_path = user_dir.join(&id);

    if !file_path.exists() {
        return AssetsError::NotFound(id).into_response();
    }

    if let Err(e) = validate_file_in_user_directory(&file_path, &user_dir) {
        return e.into_response();
    }

    if let Err(e) = fs::remove_file(&file_path)
        .await
        .map_err(|e| AssetsError::IoError(format!("Failed to delete file: {e}")))
    {
        error!("Failed to delete font file: {}", e);
        return e.into_response();
    }

    // Also remove associated license file if present
    let extension = id.rsplit('.').next().unwrap_or("");
    let license_path = file_path.with_extension(format!("{extension}.license"));
    if license_path.exists() {
        if let Err(e) = fs::remove_file(&license_path).await {
            warn!("Failed to delete font license file: {}", e);
        }
    }

    // Invalidate the font cache entry so re-uploads with the same name get a fresh parse.
    let asset_path = format!("samples/fonts/user/{id}");
    streamkit_nodes::video::compositor::overlay::invalidate_font_cache_entry(
        &asset_path,
        &app_state.asset_root,
    );

    info!("Deleted font asset: {}", id);
    StatusCode::NO_CONTENT.into_response()
}

/// Serve a font asset file by scope and ID.
async fn serve_font_asset_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((scope, id)): Path<(String, String)>,
) -> impl IntoResponse {
    use axum::http::header;

    let perms = get_permissions(&headers, &app_state);

    if id.contains("..") || id.contains('/') || id.contains('\\') {
        return AssetsError::InvalidFilename("Invalid characters in filename".to_string())
            .into_response();
    }

    if scope != "user" && scope != "system" {
        return AssetsError::InvalidFilename(
            "Invalid scope: must be 'user' or 'system'".to_string(),
        )
        .into_response();
    }

    let file_path = app_state.asset_root.join("samples/fonts").join(&scope).join(&id);
    let asset_path_str = format!("samples/fonts/{scope}/{id}");

    let extension = file_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    if !ALLOWED_FONT_FORMATS.contains(&extension.as_str()) {
        return AssetsError::InvalidFormat(format!("Not an allowed font format: {extension}"))
            .into_response();
    }

    if !perms.is_asset_allowed(&asset_path_str) {
        return AssetsError::Forbidden.into_response();
    }

    if !file_path.exists() {
        return AssetsError::NotFound(id).into_response();
    }

    let content_type = match extension.as_str() {
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        _ => "application/octet-stream",
    };

    match fs::read(&file_path).await {
        Ok(data) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, content_type.to_string()),
                (header::CACHE_CONTROL, "public, must-revalidate".to_string()),
            ],
            data,
        )
            .into_response(),
        Err(e) => {
            error!("Failed to read font file {:?}: {}", file_path, e);
            AssetsError::IoError(format!("Failed to read file: {e}")).into_response()
        },
    }
}

/// Create router for font asset endpoints
pub fn font_assets_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/api/v1/assets/fonts",
            get(list_font_assets_handler)
                .post(upload_font_asset_handler)
                .layer(DefaultBodyLimit::max(MAX_FONT_FILE_SIZE)),
        )
        .route("/api/v1/assets/fonts/file/{scope}/{id}", get(serve_font_asset_handler))
        .route("/api/v1/assets/fonts/{id}", delete(delete_font_asset_handler))
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

#[cfg(test)]
// `unwrap` / `expect` are idiomatic in tests where the panic IS the assertion.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::{AuthMode, Config};
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Method, Request, StatusCode};
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn make_state(root: &std::path::Path) -> Arc<AppState> {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        config.asset_root = Some(root.to_owned());
        crate::server::create_app_state(config, None)
    }

    fn make_viewer_state(root: &std::path::Path) -> Arc<AppState> {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        config.permissions.default_role = "viewer".to_string();
        config.asset_root = Some(root.to_owned());
        crate::server::create_app_state(config, None)
    }

    /// Build a multipart/form-data body with a single named file part.
    fn build_multipart_body(
        boundary: &str,
        field_name: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{field_name}\"; \
                 filename=\"{filename}\"\r\n"
            )
            .as_bytes(),
        );
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

    /// Bytes for a tiny 1×1 transparent PNG (header + IHDR + IDAT + IEND).
    fn tiny_png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes: Vec<u8> = Vec::new();
        let pixel = [0u8, 0, 0, 0];
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&pixel, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn tiny_jpeg() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes: Vec<u8> = Vec::new();
        let pixel = [0u8, 0, 0];
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 80)
            .write_image(&pixel, 1, 1, image::ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn tiny_gif() -> Vec<u8> {
        let mut bytes: Vec<u8> = Vec::new();
        let pixel = [0u8, 0, 0, 0];
        image::codecs::gif::GifEncoder::new(&mut bytes)
            .encode(&pixel, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn tiny_webp() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes: Vec<u8> = Vec::new();
        let pixel = [0u8, 0, 0, 0];
        image::codecs::webp::WebPEncoder::new_lossless(&mut bytes)
            .write_image(&pixel, 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }

    fn tiny_svg() -> Vec<u8> {
        b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"10\"/>".to_vec()
    }

    /// 4-byte TTF magic + zero padding to clear the 4-byte minimum length check.
    /// Exercises the header sniff in `process_font_entry`, not full font parsing.
    fn fake_ttf() -> Vec<u8> {
        let mut v = vec![0x00, 0x01, 0x00, 0x00];
        v.extend_from_slice(&[0u8; 16]);
        v
    }

    /// 4-byte OTF magic + zero padding to clear the 4-byte minimum length check.
    /// Exercises the header sniff in `process_font_entry`, not full font parsing.
    fn fake_otf() -> Vec<u8> {
        let mut v = b"OTTO".to_vec();
        v.extend_from_slice(&[0u8; 16]);
        v
    }

    #[test]
    fn validate_audio_accepts_each_allowed_extension() {
        for ext in ALLOWED_AUDIO_FORMATS {
            let name = format!("clip.{ext}");
            let parsed = validate_audio_filename(&name).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(parsed, *ext);
        }
    }

    #[test]
    fn validate_audio_is_case_insensitive_on_extension() {
        assert_eq!(validate_audio_filename("clip.OPUS").unwrap(), "opus");
        assert_eq!(validate_audio_filename("clip.Mp3").unwrap(), "mp3");
    }

    #[test]
    fn validate_audio_rejects_empty() {
        assert!(matches!(validate_audio_filename(""), Err(AssetsError::InvalidFilename(_))));
    }

    #[test]
    fn validate_audio_filename_length_boundary() {
        let suffix = ".opus";
        let at_limit = format!("{}{}", "a".repeat(MAX_FILENAME_LENGTH - suffix.len()), suffix);
        assert_eq!(at_limit.len(), MAX_FILENAME_LENGTH);
        assert_eq!(validate_audio_filename(&at_limit).unwrap(), "opus");

        let over_limit =
            format!("{}{}", "a".repeat(MAX_FILENAME_LENGTH - suffix.len() + 1), suffix);
        assert_eq!(over_limit.len(), MAX_FILENAME_LENGTH + 1);
        assert!(matches!(
            validate_audio_filename(&over_limit),
            Err(AssetsError::InvalidFilename(_))
        ));
    }

    #[test]
    fn validate_audio_rejects_path_traversal_and_separators() {
        for bad in ["../etc/passwd", "a/b.opus", "a\\b.opus", "..opus"] {
            assert!(
                matches!(validate_audio_filename(bad), Err(AssetsError::InvalidFilename(_))),
                "expected reject for {bad}"
            );
        }
    }

    #[test]
    fn validate_audio_rejects_disallowed_extension() {
        assert!(matches!(validate_audio_filename("clip.exe"), Err(AssetsError::InvalidFormat(_))));
    }

    #[test]
    fn validate_image_accepts_each_allowed_extension() {
        for ext in ALLOWED_IMAGE_FORMATS {
            let name = format!("pic.{ext}");
            let parsed = validate_image_filename(&name).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(parsed, *ext);
        }
    }

    #[test]
    fn validate_image_rejects_extensionless() {
        // "noext" has no '.' at all — must be rejected even though rsplit
        // would otherwise return the whole string.
        assert!(matches!(validate_image_filename("noext"), Err(AssetsError::InvalidFilename(_))));
    }

    #[test]
    fn validate_image_rejects_path_traversal() {
        for bad in ["../a.png", "sub/a.png", "sub\\a.png"] {
            assert!(matches!(validate_image_filename(bad), Err(AssetsError::InvalidFilename(_))));
        }
    }

    #[test]
    fn validate_image_rejects_wrong_extension() {
        assert!(matches!(validate_image_filename("clip.opus"), Err(AssetsError::InvalidFormat(_))));
    }

    #[test]
    fn validate_font_accepts_each_allowed_extension() {
        for ext in ALLOWED_FONT_FORMATS {
            let name = format!("face.{ext}");
            let parsed = validate_font_filename(&name).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!(parsed, *ext);
        }
    }

    #[test]
    fn validate_font_rejects_no_extension_and_traversal() {
        assert!(matches!(validate_font_filename("noext"), Err(AssetsError::InvalidFilename(_))));
        assert!(matches!(
            validate_font_filename("../font.ttf"),
            Err(AssetsError::InvalidFilename(_))
        ));
    }

    #[test]
    fn validate_font_rejects_wrong_extension() {
        assert!(matches!(validate_font_filename("face.exe"), Err(AssetsError::InvalidFormat(_))));
    }

    #[test]
    fn sanitize_keeps_alphanumeric_and_safe_punct() {
        assert_eq!(sanitize_filename("Hello-World_1.opus"), "Hello-World_1.opus");
    }

    #[test]
    fn sanitize_replaces_disallowed_chars_with_underscore() {
        assert_eq!(sanitize_filename("../etc/passwd"), ".._etc_passwd");
        assert_eq!(sanitize_filename("my file (1).mp3"), "my_file__1_.mp3");
        assert_eq!(sanitize_filename("café.png"), "caf_.png");
    }

    #[test]
    fn sanitize_replaces_null_byte_with_underscore() {
        assert_eq!(sanitize_filename("a\0b.opus"), "a_b.opus");
    }

    #[test]
    fn build_upload_response_shapes_the_audio_asset() {
        let asset =
            build_upload_response("My_Sample-1.opus", "opus", std::path::Path::new("ignored"), 42);
        assert_eq!(asset.id, "My_Sample-1.opus");
        assert_eq!(asset.name, "My Sample 1");
        assert_eq!(asset.format, "opus");
        assert_eq!(asset.path, "samples/audio/user/My_Sample-1.opus");
        assert_eq!(asset.size_bytes, 42);
        assert!(!asset.is_system);
        assert!(asset.license.unwrap().contains("CC0-1.0"));
    }

    #[tokio::test]
    async fn read_license_returns_none_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("missing.license");
        assert!(read_license_file(&p).await.is_none());
    }

    #[tokio::test]
    async fn read_license_extracts_spdx_fields() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("clip.opus.license");
        // REUSE-IgnoreStart
        let body = "SPDX-License-Identifier: MIT\nSPDX-FileCopyrightText: © 2025 Test\n";
        // REUSE-IgnoreEnd
        tokio::fs::write(&p, body).await.unwrap();

        let parsed = read_license_file(&p).await.unwrap();
        assert!(parsed.contains("License: MIT"), "got: {parsed}");
        assert!(parsed.contains("Copyright: © 2025 Test"), "got: {parsed}");
    }

    #[tokio::test]
    async fn read_license_returns_none_when_no_spdx_lines() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("plain.license");
        tokio::fs::write(&p, "just some text\n").await.unwrap();
        assert!(read_license_file(&p).await.is_none());
    }

    #[tokio::test]
    async fn process_audio_entry_skips_dirs_and_license_files() {
        let tmp = TempDir::new().unwrap();
        let perms = RolePermissions::admin();
        let dir_path = tmp.path().join("inner");
        tokio::fs::create_dir(&dir_path).await.unwrap();
        assert!(process_audio_entry(dir_path, false, &perms).await.is_none());

        let lic = tmp.path().join("clip.opus.license");
        tokio::fs::write(&lic, "").await.unwrap();
        assert!(process_audio_entry(lic, false, &perms).await.is_none());
    }

    #[tokio::test]
    async fn process_audio_entry_skips_disallowed_extension() {
        let tmp = TempDir::new().unwrap();
        let perms = RolePermissions::admin();
        let p = tmp.path().join("clip.exe");
        tokio::fs::write(&p, b"data").await.unwrap();
        assert!(process_audio_entry(p, false, &perms).await.is_none());
    }

    #[tokio::test]
    async fn process_audio_entry_builds_user_asset_with_metadata() {
        let tmp = TempDir::new().unwrap();
        let perms = RolePermissions::admin();
        let p = tmp.path().join("My_Cool-Clip.opus");
        tokio::fs::write(&p, b"raw audio bytes").await.unwrap();

        let asset = process_audio_entry(p.clone(), false, &perms).await.unwrap();
        assert_eq!(asset.id, "My_Cool-Clip.opus");
        assert_eq!(asset.name, "My Cool Clip");
        assert_eq!(asset.format, "opus");
        assert_eq!(asset.path, "samples/audio/user/My_Cool-Clip.opus");
        assert_eq!(asset.size_bytes, b"raw audio bytes".len() as u64);
        assert!(!asset.is_system);
    }

    #[tokio::test]
    async fn process_audio_entry_marks_system_assets() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("sys.flac");
        tokio::fs::write(&p, b"x").await.unwrap();
        let asset = process_audio_entry(p, true, &RolePermissions::admin()).await.unwrap();
        assert!(asset.is_system);
        assert_eq!(asset.path, "samples/audio/system/sys.flac");
    }

    #[tokio::test]
    async fn process_audio_entry_filters_by_permissions() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("denied.opus");
        tokio::fs::write(&p, b"x").await.unwrap();
        let mut perms = RolePermissions::admin();
        perms.allowed_assets.clear(); // deny everything
        assert!(process_audio_entry(p, false, &perms).await.is_none());
    }

    #[tokio::test]
    async fn process_audio_entry_attaches_license_when_sidecar_present() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("clip.opus");
        tokio::fs::write(&p, b"x").await.unwrap();
        // REUSE-IgnoreStart
        let lic_text = "SPDX-License-Identifier: CC0-1.0\nSPDX-FileCopyrightText: © 2025 A\n";
        // REUSE-IgnoreEnd
        tokio::fs::write(tmp.path().join("clip.opus.license"), lic_text).await.unwrap();
        let asset = process_audio_entry(p, false, &RolePermissions::admin()).await.unwrap();
        assert!(asset.license.as_deref().unwrap().contains("CC0-1.0"));
    }

    #[tokio::test]
    async fn process_image_entry_returns_dimensions_for_png() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("pic.png");
        tokio::fs::write(&p, tiny_png()).await.unwrap();
        let asset = process_image_entry(p, false, &RolePermissions::admin()).await.unwrap();
        assert_eq!(asset.format, "png");
        assert_eq!(asset.width, 1);
        assert_eq!(asset.height, 1);
        assert_eq!(asset.path, "samples/images/user/pic.png");
        assert!(!asset.is_system);
    }

    #[tokio::test]
    async fn process_image_entry_returns_dimensions_for_svg() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("vec.svg");
        tokio::fs::write(&p, tiny_svg()).await.unwrap();
        let asset = process_image_entry(p, false, &RolePermissions::admin()).await.unwrap();
        assert_eq!(asset.format, "svg");
        assert_eq!(asset.width, 10);
        assert_eq!(asset.height, 10);
    }

    #[tokio::test]
    async fn process_image_entry_skips_dirs_license_and_wrong_ext() {
        let tmp = TempDir::new().unwrap();
        let perms = RolePermissions::admin();

        let sub = tmp.path().join("dir");
        tokio::fs::create_dir(&sub).await.unwrap();
        assert!(process_image_entry(sub, false, &perms).await.is_none());

        let lic = tmp.path().join("a.png.license");
        tokio::fs::write(&lic, "").await.unwrap();
        assert!(process_image_entry(lic, false, &perms).await.is_none());

        let other = tmp.path().join("doc.txt");
        tokio::fs::write(&other, "").await.unwrap();
        assert!(process_image_entry(other, false, &perms).await.is_none());
    }

    #[tokio::test]
    async fn process_image_entry_returns_none_for_corrupt_image() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("bad.png");
        tokio::fs::write(&p, b"not a png").await.unwrap();
        assert!(process_image_entry(p, false, &RolePermissions::admin()).await.is_none());
    }

    #[tokio::test]
    async fn process_font_entry_builds_asset_and_respects_system_flag() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("My-Face.ttf");
        tokio::fs::write(&p, fake_ttf()).await.unwrap();
        let asset = process_font_entry(p, true, &RolePermissions::admin()).await.unwrap();
        assert_eq!(asset.id, "My-Face.ttf");
        assert_eq!(asset.format, "ttf");
        assert_eq!(asset.name, "My Face");
        assert!(asset.is_system);
        assert_eq!(asset.path, "samples/fonts/system/My-Face.ttf");
    }

    #[tokio::test]
    async fn process_font_entry_skips_dirs_and_wrong_extension() {
        let tmp = TempDir::new().unwrap();
        let perms = RolePermissions::admin();
        let sub = tmp.path().join("dir");
        tokio::fs::create_dir(&sub).await.unwrap();
        assert!(process_font_entry(sub, false, &perms).await.is_none());

        let bad = tmp.path().join("script.exe");
        tokio::fs::write(&bad, b"").await.unwrap();
        assert!(process_font_entry(bad, false, &perms).await.is_none());
    }

    #[tokio::test]
    async fn scan_audio_directory_returns_empty_for_missing_dir() {
        let tmp = TempDir::new().unwrap();
        let assets =
            scan_audio_directory(&tmp.path().join("nope"), false, &RolePermissions::admin())
                .await
                .unwrap();
        assert!(assets.is_empty());
    }

    #[tokio::test]
    async fn scan_audio_directory_collects_and_filters() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        for name in ["a.opus", "b.flac", "junk.txt"] {
            tokio::fs::write(dir.join(name), b"x").await.unwrap();
        }
        let assets = scan_audio_directory(&dir, false, &RolePermissions::admin()).await.unwrap();
        // The handler-level list_assets sorts; scan_*_directory itself
        // does not. Verify both audio files surfaced and the .txt was dropped.
        let mut ids: Vec<_> = assets.iter().map(|a| a.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, vec!["a.opus".to_string(), "b.flac".to_string()]);
    }

    #[tokio::test]
    async fn scan_image_directory_filters_to_allowed_formats() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        tokio::fs::write(dir.join("pic.png"), tiny_png()).await.unwrap();
        tokio::fs::write(dir.join("doc.txt"), b"text").await.unwrap();
        let assets = scan_image_directory(&dir, false, &RolePermissions::admin()).await.unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].format, "png");
    }

    #[tokio::test]
    async fn scan_font_directory_filters_to_allowed_formats() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().to_path_buf();
        tokio::fs::write(dir.join("face.ttf"), fake_ttf()).await.unwrap();
        tokio::fs::write(dir.join("notes.txt"), b"x").await.unwrap();
        let assets = scan_font_directory(&dir, false, &RolePermissions::admin()).await.unwrap();
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0].format, "ttf");
    }

    #[tokio::test]
    async fn create_license_sidecar_writes_default_spdx_text() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("clip.opus");
        tokio::fs::write(&file, b"x").await.unwrap();
        create_license_sidecar(&file, "opus").await;
        let written = tokio::fs::read_to_string(file.with_extension("opus.license")).await.unwrap();
        // REUSE-IgnoreStart
        assert!(written.contains("SPDX-License-Identifier: CC0-1.0"));
        assert!(written.contains("SPDX-FileCopyrightText:"));
        // REUSE-IgnoreEnd
    }

    #[tokio::test]
    async fn validate_file_in_user_directory_accepts_child() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        tokio::fs::create_dir_all(&user_dir).await.unwrap();
        let child = user_dir.join("file.opus");
        tokio::fs::write(&child, b"x").await.unwrap();
        assert!(validate_file_in_user_directory(&child, &user_dir).is_ok());
    }

    #[tokio::test]
    async fn validate_file_in_user_directory_rejects_outside() {
        let tmp = TempDir::new().unwrap();
        let user_dir = tmp.path().join("user");
        tokio::fs::create_dir_all(&user_dir).await.unwrap();
        let outside = tmp.path().join("escape.opus");
        tokio::fs::write(&outside, b"x").await.unwrap();
        let err = validate_file_in_user_directory(&outside, &user_dir).unwrap_err();
        assert!(matches!(err, AssetsError::Forbidden));
    }

    #[tokio::test]
    async fn delete_audio_files_removes_asset_and_license() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("clip.opus");
        let lic = tmp.path().join("clip.opus.license");
        tokio::fs::write(&file, b"x").await.unwrap();
        tokio::fs::write(&lic, b"x").await.unwrap();
        delete_audio_files(&file, "opus").await.unwrap();
        assert!(!file.exists());
        assert!(!lic.exists());
    }

    #[tokio::test]
    async fn delete_audio_files_succeeds_without_license_sidecar() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("clip.mp3");
        tokio::fs::write(&file, b"x").await.unwrap();
        delete_audio_files(&file, "mp3").await.unwrap();
        assert!(!file.exists());
    }

    #[tokio::test]
    async fn delete_audio_files_reports_io_error_for_missing_target() {
        let tmp = TempDir::new().unwrap();
        let nope = tmp.path().join("ghost.opus");
        let err = delete_audio_files(&nope, "opus").await.unwrap_err();
        assert!(matches!(err, AssetsError::IoError(_)));
    }

    #[tokio::test]
    async fn assets_error_maps_to_expected_status_codes() {
        let cases: [(AssetsError, StatusCode); 8] = [
            (AssetsError::IoError("x".into()), StatusCode::INTERNAL_SERVER_ERROR),
            (AssetsError::InvalidFilename("x".into()), StatusCode::BAD_REQUEST),
            (AssetsError::InvalidFormat("x".into()), StatusCode::BAD_REQUEST),
            (AssetsError::InvalidRequest("x".into()), StatusCode::BAD_REQUEST),
            (AssetsError::FileTooLarge(123), StatusCode::PAYLOAD_TOO_LARGE),
            (AssetsError::FileExists("a".into()), StatusCode::CONFLICT),
            (AssetsError::NotFound("a".into()), StatusCode::NOT_FOUND),
            (AssetsError::Forbidden, StatusCode::FORBIDDEN),
        ];
        for (err, expected) in cases {
            let resp = err.into_response();
            assert_eq!(resp.status(), expected);
        }
    }

    #[test]
    fn assets_error_display_includes_payload() {
        let msg = AssetsError::FileTooLarge(42).to_string();
        assert!(msg.contains("42"));
        let msg = AssetsError::NotFound("x.opus".into()).to_string();
        assert!(msg.contains("x.opus"));
    }

    #[tokio::test]
    async fn list_audio_assets_returns_empty_when_no_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/assets/audio").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp.into_body()).await;
        assert_eq!(body, "[]");
    }

    #[tokio::test]
    async fn list_audio_assets_returns_sorted_user_and_system() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/audio/system")).await.unwrap();
        tokio::fs::create_dir_all(root.join("samples/audio/user")).await.unwrap();
        tokio::fs::write(root.join("samples/audio/system/zulu.opus"), b"x").await.unwrap();
        tokio::fs::write(root.join("samples/audio/user/alpha.flac"), b"x").await.unwrap();

        let state = make_state(root);
        let app = assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/assets/audio").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Vec<AudioAsset> =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].name, "alpha");
        assert_eq!(body[1].name, "zulu");
        assert!(!body[0].is_system);
        assert!(body[1].is_system);
    }

    #[tokio::test]
    async fn upload_audio_happy_path_writes_file_and_sidecar() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = assets_router().with_state(state);

        let boundary = "boundXYZ";
        let body = build_multipart_body(boundary, "file", "fresh.opus", b"some audio bytes");
        let req = multipart_request(Method::POST, "/api/v1/assets/audio", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let asset: AudioAsset =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert_eq!(asset.id, "fresh.opus");
        assert!(tokio::fs::try_exists(root.join("samples/audio/user/fresh.opus")).await.unwrap());
        assert!(tokio::fs::try_exists(root.join("samples/audio/user/fresh.opus.license"))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn upload_audio_returns_409_on_duplicate() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/audio/user")).await.unwrap();
        tokio::fs::write(root.join("samples/audio/user/dup.mp3"), b"existing").await.unwrap();

        let state = make_state(root);
        let app = assets_router().with_state(state);
        let boundary = "b1";
        let body = build_multipart_body(boundary, "file", "dup.mp3", b"newer");
        let req = multipart_request(Method::POST, "/api/v1/assets/audio", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let on_disk = tokio::fs::read(root.join("samples/audio/user/dup.mp3")).await.unwrap();
        assert_eq!(on_disk, b"existing");
    }

    #[tokio::test]
    async fn upload_audio_rejects_disallowed_extension() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = assets_router().with_state(state);
        let boundary = "b1";
        let body = build_multipart_body(boundary, "file", "evil.exe", b"x");
        let req = multipart_request(Method::POST, "/api/v1/assets/audio", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!root.join("samples/audio/user/evil.exe").exists());
    }

    #[tokio::test]
    async fn upload_audio_forbidden_when_role_lacks_permission() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_viewer_state(root);
        let app = assets_router().with_state(state);
        let boundary = "b1";
        let body = build_multipart_body(boundary, "file", "x.opus", b"x");
        let req = multipart_request(Method::POST, "/api/v1/assets/audio", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(!root.join("samples/audio/user/x.opus").exists());
    }

    #[tokio::test]
    async fn delete_audio_404_when_missing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/audio/user")).await.unwrap();
        let state = make_state(root);
        let app = assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/audio/ghost.opus")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_audio_removes_file_returns_204() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/audio/user")).await.unwrap();
        tokio::fs::write(root.join("samples/audio/user/bye.opus"), b"x").await.unwrap();
        tokio::fs::write(root.join("samples/audio/user/bye.opus.license"), b"x").await.unwrap();

        let state = make_state(root);
        let app = assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/audio/bye.opus")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!root.join("samples/audio/user/bye.opus").exists());
        assert!(!root.join("samples/audio/user/bye.opus.license").exists());
    }

    #[tokio::test]
    async fn delete_audio_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_viewer_state(root);
        let app = assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/audio/anything.opus")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn list_image_assets_returns_empty_when_no_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/assets/images").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp.into_body()).await;
        assert_eq!(body, "[]");
    }

    #[tokio::test]
    async fn list_image_assets_returns_sorted_with_dimensions() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/images/user")).await.unwrap();
        tokio::fs::write(root.join("samples/images/user/banner.png"), tiny_png()).await.unwrap();
        tokio::fs::write(root.join("samples/images/user/aero.svg"), tiny_svg()).await.unwrap();

        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/assets/images").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body: Vec<ImageAsset> =
            serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert_eq!(body.len(), 2);
        assert_eq!(body[0].name, "aero");
        assert_eq!(body[0].width, 10);
        assert_eq!(body[1].name, "banner");
        assert_eq!(body[1].width, 1);
    }

    #[tokio::test]
    async fn upload_image_happy_path_for_each_raster_format() {
        let cases: [(&str, Vec<u8>); 4] = [
            ("a.png", tiny_png()),
            ("a.jpg", tiny_jpeg()),
            ("a.gif", tiny_gif()),
            ("a.webp", tiny_webp()),
        ];
        for (filename, bytes) in cases {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            let state = make_state(root);
            let app = image_assets_router().with_state(state);
            let boundary = "imgBoundary";
            let body = build_multipart_body(boundary, "file", filename, &bytes);
            let req = multipart_request(Method::POST, "/api/v1/assets/images", boundary, body);
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{filename}");
            let asset: ImageAsset =
                serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
            assert_eq!(asset.id, filename);
            assert!(asset.width >= 1);
        }
    }

    #[tokio::test]
    async fn upload_image_rejects_corrupt_png_and_deletes_partial() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let boundary = "imgB";
        let body = build_multipart_body(boundary, "file", "bad.png", b"not an image");
        let req = multipart_request(Method::POST, "/api/v1/assets/images", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!root.join("samples/images/user/bad.png").exists());
    }

    #[tokio::test]
    async fn upload_image_rejects_invalid_svg() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let boundary = "imgB";
        let body = build_multipart_body(boundary, "file", "bad.svg", b"not svg");
        let req = multipart_request(Method::POST, "/api/v1/assets/images", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!root.join("samples/images/user/bad.svg").exists());
    }

    #[tokio::test]
    async fn upload_image_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_viewer_state(root);
        let app = image_assets_router().with_state(state);
        let boundary = "b";
        let body = build_multipart_body(boundary, "file", "x.png", &tiny_png());
        let req = multipart_request(Method::POST, "/api/v1/assets/images", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_image_404_when_missing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/images/ghost.png")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_image_returns_204_and_removes_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/images/user")).await.unwrap();
        tokio::fs::write(root.join("samples/images/user/bye.png"), tiny_png()).await.unwrap();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/images/bye.png")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!root.join("samples/images/user/bye.png").exists());
    }

    #[tokio::test]
    async fn delete_image_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_viewer_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/images/anything.png")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn serve_image_returns_correct_content_types() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/images/user")).await.unwrap();
        let cases: [(&str, Vec<u8>, &str); 4] = [
            ("pic.png", tiny_png(), "image/png"),
            ("pic.jpg", tiny_jpeg(), "image/jpeg"),
            ("pic.gif", tiny_gif(), "image/gif"),
            ("pic.webp", tiny_webp(), "image/webp"),
        ];
        for (name, bytes, expected) in cases {
            tokio::fs::write(root.join(format!("samples/images/user/{name}")), &bytes)
                .await
                .unwrap();
            let state = make_state(root);
            let app = image_assets_router().with_state(state);
            let req = Request::builder()
                .uri(format!("/api/v1/assets/images/file/user/{name}"))
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{name}");
            let ct =
                resp.headers().get(header::CONTENT_TYPE).unwrap().to_str().unwrap().to_string();
            assert_eq!(ct, expected, "{name}");
        }
    }

    #[tokio::test]
    async fn serve_image_svg_emits_security_headers() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/images/user")).await.unwrap();
        tokio::fs::write(root.join("samples/images/user/safe.svg"), tiny_svg()).await.unwrap();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/images/file/user/safe.svg")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let h = resp.headers();
        assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "image/svg+xml");
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert!(h.get(header::CONTENT_SECURITY_POLICY).is_some());
        assert!(h.get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn serve_image_svgz_emits_gzip_content_encoding() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/images/user")).await.unwrap();
        tokio::fs::write(root.join("samples/images/user/safe.svgz"), &[0u8; 32]).await.unwrap();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/images/file/user/safe.svgz")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(resp.headers().get(header::CONTENT_ENCODING).unwrap(), "gzip");
    }

    #[tokio::test]
    async fn serve_image_404_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/images/file/user/missing.png")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serve_image_rejects_invalid_scope() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/images/file/admin/foo.png")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_image_rejects_disallowed_format() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = image_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/images/file/user/foo.exe")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn list_font_assets_returns_empty_when_no_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/assets/fonts").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_string(resp.into_body()).await;
        assert_eq!(body, "[]");
    }

    #[tokio::test]
    async fn upload_font_happy_path_ttf() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let boundary = "fb";
        let body = build_multipart_body(boundary, "file", "myface.ttf", &fake_ttf());
        let req = multipart_request(Method::POST, "/api/v1/assets/fonts", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let asset: FontAsset = serde_json::from_slice(&body_bytes(resp.into_body()).await).unwrap();
        assert_eq!(asset.id, "myface.ttf");
        assert_eq!(asset.format, "ttf");
        assert!(tokio::fs::try_exists(root.join("samples/fonts/user/myface.ttf")).await.unwrap());
        assert!(tokio::fs::try_exists(root.join("samples/fonts/user/myface.ttf.license"))
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn upload_font_happy_path_otf() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let boundary = "fb";
        let body = build_multipart_body(boundary, "file", "myface.otf", &fake_otf());
        let req = multipart_request(Method::POST, "/api/v1/assets/fonts", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn upload_font_rejects_invalid_magic_bytes_and_cleans_up() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let boundary = "fb";
        let body = build_multipart_body(boundary, "file", "fake.ttf", b"NOTAFONTHEADERAAAAAAAAAA");
        let req = multipart_request(Method::POST, "/api/v1/assets/fonts", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(!root.join("samples/fonts/user/fake.ttf").exists());
        assert!(!root.join("samples/fonts/user/fake.ttf.license").exists());
    }

    #[tokio::test]
    async fn upload_font_rejects_too_small_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let boundary = "fb";
        let body = build_multipart_body(boundary, "file", "tiny.ttf", b"ab");
        let req = multipart_request(Method::POST, "/api/v1/assets/fonts", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn upload_font_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_viewer_state(root);
        let app = font_assets_router().with_state(state);
        let boundary = "fb";
        let body = build_multipart_body(boundary, "file", "x.ttf", &fake_ttf());
        let req = multipart_request(Method::POST, "/api/v1/assets/fonts", boundary, body);
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delete_font_returns_204_and_removes_files() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/fonts/user")).await.unwrap();
        tokio::fs::write(root.join("samples/fonts/user/bye.ttf"), fake_ttf()).await.unwrap();
        tokio::fs::write(root.join("samples/fonts/user/bye.ttf.license"), b"x").await.unwrap();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/fonts/bye.ttf")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        assert!(!root.join("samples/fonts/user/bye.ttf").exists());
        assert!(!root.join("samples/fonts/user/bye.ttf.license").exists());
    }

    #[tokio::test]
    async fn delete_font_404_when_missing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/fonts/ghost.ttf")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_font_forbidden_for_viewer() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_viewer_state(root);
        let app = font_assets_router().with_state(state);
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/api/v1/assets/fonts/anything.ttf")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn serve_font_returns_correct_content_types() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        tokio::fs::create_dir_all(root.join("samples/fonts/user")).await.unwrap();
        let cases: [(&str, Vec<u8>, &str); 2] =
            [("face.ttf", fake_ttf(), "font/ttf"), ("face.otf", fake_otf(), "font/otf")];
        for (name, bytes, expected) in cases {
            tokio::fs::write(root.join(format!("samples/fonts/user/{name}")), &bytes)
                .await
                .unwrap();
            let state = make_state(root);
            let app = font_assets_router().with_state(state);
            let req = Request::builder()
                .uri(format!("/api/v1/assets/fonts/file/user/{name}"))
                .body(Body::empty())
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK, "{name}");
            let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
            assert_eq!(ct, expected, "{name}");
        }
    }

    #[tokio::test]
    async fn serve_font_404_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/fonts/file/user/missing.ttf")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn serve_font_rejects_invalid_scope() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/fonts/file/system_admin/x.ttf")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn serve_font_rejects_disallowed_format() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let state = make_state(root);
        let app = font_assets_router().with_state(state);
        let req = Request::builder()
            .uri("/api/v1/assets/fonts/file/user/x.exe")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
