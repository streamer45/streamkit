// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tracing::{debug, error, info, warn};

use crate::permissions::Permissions as RolePermissions;
use crate::role_extractor::get_permissions;
use crate::state::AppState;
use streamkit_api::{SamplePipeline, SavePipelineRequest};

// Security limits
const MAX_FILE_SIZE: usize = 1024 * 1024; // 1MB
const MAX_FILENAME_LENGTH: usize = 255;

/// Validates a filename for security
fn validate_filename(filename: &str) -> Result<(), SamplesError> {
    // Check length
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(SamplesError::InvalidFilename("Filename too long".to_string()));
    }

    // Check if empty
    if filename.is_empty() {
        return Err(SamplesError::InvalidFilename("Filename cannot be empty".to_string()));
    }

    // Check for path traversal attempts
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(SamplesError::InvalidFilename("Invalid characters in filename".to_string()));
    }

    // Check extension (case-insensitive)
    let has_valid_extension = std::path::Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"));

    if !has_valid_extension {
        return Err(SamplesError::InvalidFilename(
            "File must have .yml or .yaml extension".to_string(),
        ));
    }

    Ok(())
}

/// Generates a name from a filename as a fallback
fn filename_to_name(filename: &str) -> String {
    filename
        .trim_end_matches(".yml")
        .trim_end_matches(".yaml")
        .replace('_', " ")
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Lists all available oneshot sample pipelines
async fn list_samples_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);
    if !perms.list_samples {
        return SamplesError::Forbidden.into_response();
    }
    match list_samples(&app_state, &perms).await {
        Ok(samples) => {
            // Filter to only oneshot pipelines
            let oneshot_samples: Vec<SamplePipeline> =
                samples.into_iter().filter(|s| s.mode == "oneshot").collect();
            info!("Listed {} oneshot sample pipelines", oneshot_samples.len());
            Json(oneshot_samples).into_response()
        },
        Err(e) => {
            error!("Failed to list samples: {}", e);
            e.into_response()
        },
    }
}

/// Lists all available dynamic sample pipelines
async fn list_dynamic_samples_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);
    if !perms.list_samples {
        return SamplesError::Forbidden.into_response();
    }
    match list_samples(&app_state, &perms).await {
        Ok(samples) => {
            // Filter to only dynamic pipelines
            let dynamic_samples: Vec<SamplePipeline> =
                samples.into_iter().filter(|s| s.mode == "dynamic").collect();
            info!("Listed {} dynamic sample pipelines", dynamic_samples.len());
            Json(dynamic_samples).into_response()
        },
        Err(e) => {
            error!("Failed to list dynamic samples: {}", e);
            e.into_response()
        },
    }
}

async fn list_samples(
    app_state: &AppState,
    perms: &RolePermissions,
) -> Result<Vec<SamplePipeline>, SamplesError> {
    let base_path = PathBuf::from(&app_state.config.server.samples_dir);
    let mut samples = Vec::new();

    // Load system samples from oneshot/
    let oneshot_path = base_path.join("oneshot");
    if oneshot_path.exists() {
        samples.extend(load_samples_from_dir(&oneshot_path, true, "oneshot").await?);
    }

    // Load system samples from dynamic/
    let dynamic_path = base_path.join("dynamic");
    if dynamic_path.exists() {
        samples.extend(load_samples_from_dir(&dynamic_path, true, "dynamic").await?);
    }

    // Load user samples from user/
    let user_path = base_path.join("user");
    if user_path.exists() {
        samples.extend(load_samples_from_dir(&user_path, false, "user").await?);
    }

    // Load demo samples from demo/
    let demo_path = base_path.join("demo");
    if demo_path.exists() {
        samples.extend(load_samples_from_dir(&demo_path, true, "demo").await?);
    }

    // Filter samples based on permissions
    let filtered_samples: Vec<SamplePipeline> = samples
        .into_iter()
        .filter(|sample| {
            // Permission matching for samples is always evaluated against paths relative to
            // `[server].samples_dir`. `sample.id` is already namespaced like `oneshot/foo`,
            // `dynamic/bar`, `user/baz`.
            //
            // Try both `.yml` and `.yaml` to keep allowlists ergonomic.
            let path_yml = format!("{}.yml", sample.id);
            let path_yaml = format!("{}.yaml", sample.id);

            let allowed = perms.is_sample_allowed(&path_yml) || perms.is_sample_allowed(&path_yaml);

            debug!(
                sample_id = %sample.id,
                path_yml = %path_yml,
                allowed = allowed,
                "Checking sample permission"
            );

            allowed
        })
        .collect();

    Ok(filtered_samples)
}

/// Check if a file has a valid YAML extension (.yml or .yaml, case-insensitive)
fn has_yaml_extension(filename: &str) -> bool {
    std::path::Path::new(filename)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml"))
}

/// Parse pipeline YAML and extract metadata (name, description, mode)
fn parse_pipeline_metadata(
    yaml: &str,
    path: &std::path::Path,
) -> (Option<String>, Option<String>, streamkit_api::EngineMode) {
    serde_saphyr::from_str::<streamkit_api::yaml::UserPipeline>(yaml).map_or_else(
        |e| {
            warn!("Failed to parse pipeline metadata from {}: {}", path.display(), e);
            (None, None, streamkit_api::EngineMode::default())
        },
        |user_pipeline| {
            use streamkit_api::yaml::UserPipeline;
            match user_pipeline {
                UserPipeline::Steps { name, description, mode, .. }
                | UserPipeline::Dag { name, description, mode, .. } => (name, description, mode),
            }
        },
    )
}

/// Convert engine mode enum to string representation
fn mode_to_string(mode: streamkit_api::EngineMode) -> String {
    match mode {
        streamkit_api::EngineMode::OneShot => "oneshot".to_string(),
        streamkit_api::EngineMode::Dynamic => "dynamic".to_string(),
    }
}

async fn load_samples_from_dir(
    dir: &PathBuf,
    is_system: bool,
    subdir: &str,
) -> Result<Vec<SamplePipeline>, SamplesError> {
    let mut samples = Vec::new();
    let mut entries = fs::read_dir(dir).await?;

    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(filename) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if !has_yaml_extension(filename) {
            continue;
        }

        // Check file size before reading
        let metadata = fs::metadata(&path).await?;
        if metadata.len() > MAX_FILE_SIZE as u64 {
            warn!("Skipping file {} - exceeds size limit", path.display());
            continue;
        }

        // Read and parse file content
        match fs::read_to_string(&path).await {
            Ok(yaml) => {
                let (name, description, mode) = parse_pipeline_metadata(&yaml, &path);

                // Generate ID with directory prefix to ensure uniqueness across directories
                let base_filename = filename.trim_end_matches(".yml").trim_end_matches(".yaml");
                let id = format!("{subdir}/{base_filename}");

                // Use metadata from schema, or fallback to filename-based name
                let name = name.unwrap_or_else(|| filename_to_name(filename));
                let description = description.unwrap_or_default();

                // Detect if this is a fragment (lacks name, description, or mode in YAML)
                let is_fragment = name == filename_to_name(filename) && description.is_empty();

                samples.push(SamplePipeline {
                    id,
                    name,
                    description,
                    yaml,
                    is_system,
                    mode: mode_to_string(mode),
                    is_fragment,
                });
            },
            Err(e) => {
                warn!("Failed to read sample file {}: {}", path.display(), e);
            },
        }
    }

    Ok(samples)
}

/// Gets a specific sample pipeline by ID
async fn get_sample_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);
    if !perms.read_samples {
        return SamplesError::Forbidden.into_response();
    }

    // Extract the filename from the ID (which may be prefixed like "oneshot/whisper-transcription")
    let filename_base = if let Some((_subdir, base)) = id.split_once('/') {
        base
    } else {
        // Fallback for legacy IDs without prefix
        &id
    };
    let filename = format!("{filename_base}.yml");

    match validate_filename(&filename) {
        Ok(()) => {},
        Err(e) => return e.into_response(),
    }

    match get_sample(&app_state, &id, &perms).await {
        Ok(sample) => {
            info!("Retrieved sample pipeline: {}", id);
            Json(sample).into_response()
        },
        Err(e) => {
            warn!("Failed to get sample {}: {}", id, e);
            e.into_response()
        },
    }
}

async fn get_sample(
    app_state: &AppState,
    id: &str,
    perms: &RolePermissions,
) -> Result<SamplePipeline, SamplesError> {
    let base_path = PathBuf::from(&app_state.config.server.samples_dir);

    // Parse ID to extract directory prefix and filename
    let (subdir_hint, filename_base) = if let Some((prefix, base)) = id.split_once('/') {
        (Some(prefix), base)
    } else {
        // Legacy ID format without prefix - try all directories
        (None, id)
    };

    // Determine which directories to search based on the prefix
    let subdirs_to_search: Vec<(&str, bool)> = if let Some(hint) = subdir_hint {
        // If prefix is present, search only that directory
        match hint {
            "oneshot" => vec![("oneshot", true)],
            "dynamic" => vec![("dynamic", true)],
            "demo" => vec![("demo", true)],
            "user" => vec![("user", false)],
            _ => {
                // Invalid prefix
                return Err(SamplesError::NotFound);
            },
        }
    } else {
        // Legacy format - try all directories for backward compatibility
        vec![("oneshot", true), ("dynamic", true), ("demo", true), ("user", false)]
    };

    for (subdir, is_system) in subdirs_to_search {
        for ext in ["yml", "yaml"] {
            let filename = format!("{filename_base}.{ext}");
            let path = base_path.join(subdir).join(&filename);

            if path.exists() {
                let metadata = fs::metadata(&path).await?;
                if metadata.len() > MAX_FILE_SIZE as u64 {
                    return Err(SamplesError::FileTooLarge);
                }

                let yaml = fs::read_to_string(&path).await?;

                // Try to parse the YAML to extract metadata
                let (name, description, mode) = parse_pipeline_metadata(&yaml, &path);

                // Use metadata from schema, or fallback to filename-based name
                let name = name.unwrap_or_else(|| filename_to_name(&filename));
                let description = description.unwrap_or_default();
                let mode_str = mode_to_string(mode);

                // Check if this sample is allowed by permissions
                let relative_path =
                    path.strip_prefix(&base_path).unwrap_or(&path).to_string_lossy().to_string();
                if !perms.is_sample_allowed(&relative_path) {
                    return Err(SamplesError::Forbidden);
                }

                // Detect if this is a fragment
                let is_fragment = name == filename_to_name(&filename) && description.is_empty();

                // Return ID with directory prefix for consistency
                let full_id = format!("{subdir}/{filename_base}");

                return Ok(SamplePipeline {
                    id: full_id,
                    name,
                    description,
                    yaml,
                    is_system,
                    mode: mode_str,
                    is_fragment,
                });
            }
        }
    }

    Err(SamplesError::NotFound)
}

/// Saves a new user pipeline
async fn save_sample_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SavePipelineRequest>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);
    // Check if user has permission to create/save samples
    if !perms.write_samples {
        return SamplesError::Forbidden.into_response();
    }

    if request.yaml.len() > MAX_FILE_SIZE {
        return SamplesError::FileTooLarge.into_response();
    }

    // Generate a safe filename from the name
    let filename = generate_safe_filename(&request.name);

    match validate_filename(&filename) {
        Ok(()) => {},
        Err(e) => return e.into_response(),
    }

    match save_sample(&app_state, &filename, &request).await {
        Ok(sample) => {
            info!("Saved user pipeline: {}", filename);
            (StatusCode::CREATED, Json(sample)).into_response()
        },
        Err(e) => {
            error!("Failed to save sample {}: {}", filename, e);
            e.into_response()
        },
    }
}

fn generate_safe_filename(name: &str) -> String {
    let safe = name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_lowercase();

    if safe.is_empty() {
        // Use timestamp as fallback. If system time is somehow before Unix epoch,
        // fall back to a static name (this should never happen in practice).
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        format!("pipeline_{timestamp}.yml")
    } else {
        format!("{safe}.yml")
    }
}

async fn save_sample(
    app_state: &AppState,
    filename: &str,
    request: &SavePipelineRequest,
) -> Result<SamplePipeline, SamplesError> {
    let base_path = PathBuf::from(&app_state.config.server.samples_dir);
    let user_dir = base_path.join("user");

    // Ensure user directory exists
    fs::create_dir_all(&user_dir).await?;

    let path = user_dir.join(filename);

    // Check if file already exists
    if path.exists() && !request.overwrite {
        return Err(SamplesError::AlreadyExists);
    }

    // Parse the YAML to add metadata fields (only for non-fragments)
    let yaml_with_metadata = if request.is_fragment {
        // For fragments, don't add name/description to YAML
        request.yaml.clone()
    } else {
        match serde_saphyr::from_str::<serde_json::Value>(&request.yaml) {
            Ok(mut value) => {
                // Add name and description to the YAML structure
                if let Some(obj) = value.as_object_mut() {
                    obj.insert("name".to_string(), serde_json::Value::String(request.name.clone()));
                    obj.insert(
                        "description".to_string(),
                        serde_json::Value::String(request.description.clone()),
                    );
                }
                serde_saphyr::to_string(&value)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
            },
            Err(_) => {
                // If parsing fails, fall back to prepending as comments
                format!(
                    "# name: {}\n# description: {}\n{}",
                    request.name, request.description, request.yaml
                )
            },
        }
    };

    fs::write(&path, &yaml_with_metadata).await?;

    let base_filename = filename.trim_end_matches(".yml").trim_end_matches(".yaml");
    // Always prefix user pipelines with "user/"
    let id = format!("user/{base_filename}");

    // Extract mode from the saved YAML
    // We only need the mode, so we discard name and description
    let (_, _, mode) = parse_pipeline_metadata(&yaml_with_metadata, &path);
    let mode_str = mode_to_string(mode);

    Ok(SamplePipeline {
        id,
        name: request.name.clone(),
        description: request.description.clone(),
        yaml: request.yaml.clone(),
        is_system: false,
        mode: mode_str,
        is_fragment: request.is_fragment,
    })
}

/// Deletes a user pipeline
async fn delete_sample_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);
    // Check if user has permission to delete samples
    if !perms.delete_samples {
        warn!(
            sample_id = %id,
            delete_samples = perms.delete_samples,
            "Blocked attempt to delete sample: permission denied"
        );
        return SamplesError::Forbidden.into_response();
    }

    // Extract the filename from the ID (which may be prefixed like "user/my-pipeline")
    let filename_base = if let Some((_subdir, base)) = id.split_once('/') {
        base
    } else {
        // Legacy ID format without prefix
        &id
    };
    let filename = format!("{filename_base}.yml");

    match validate_filename(&filename) {
        Ok(()) => {},
        Err(e) => return e.into_response(),
    }

    match delete_sample(&app_state, &id).await {
        Ok(()) => {
            info!("Deleted user pipeline: {}", id);
            StatusCode::NO_CONTENT.into_response()
        },
        Err(e) => {
            error!("Failed to delete sample {}: {}", id, e);
            e.into_response()
        },
    }
}

async fn delete_sample(app_state: &AppState, id: &str) -> Result<(), SamplesError> {
    let base_path = PathBuf::from(&app_state.config.server.samples_dir);

    // Parse ID to extract directory prefix and filename
    let (subdir_hint, filename_base) = if let Some((prefix, base)) = id.split_once('/') {
        (Some(prefix), base)
    } else {
        // Legacy ID format without prefix - assume user directory
        (Some("user"), id)
    };

    // Only allow deletion from user directory
    if subdir_hint != Some("user") {
        return Err(SamplesError::Forbidden);
    }

    let user_dir = base_path.join("user");

    for ext in ["yml", "yaml"] {
        let filename = format!("{filename_base}.{ext}");
        let path = user_dir.join(&filename);

        if path.exists() {
            fs::remove_file(&path).await?;
            return Ok(());
        }
    }

    Err(SamplesError::NotFound)
}

/// Router for sample pipeline endpoints
pub fn samples_router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/v1/samples/oneshot", get(list_samples_handler).post(save_sample_handler))
        .route(
            "/api/v1/samples/oneshot/{id}",
            get(get_sample_handler).delete(delete_sample_handler),
        )
        .route("/api/v1/samples/dynamic", get(list_dynamic_samples_handler))
}

/// Error types for sample operations
#[derive(Debug)]
enum SamplesError {
    NotFound,
    InvalidFilename(String),
    FileTooLarge,
    AlreadyExists,
    Forbidden,
    Io(std::io::Error),
}

impl std::fmt::Display for SamplesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "Sample not found"),
            Self::InvalidFilename(msg) => write!(f, "Invalid filename: {msg}"),
            Self::FileTooLarge => write!(f, "File exceeds size limit"),
            Self::AlreadyExists => write!(f, "Sample already exists"),
            Self::Forbidden => write!(f, "Access forbidden"),
            Self::Io(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl IntoResponse for SamplesError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            Self::NotFound => (StatusCode::NOT_FOUND, self.to_string()),
            Self::InvalidFilename(_) => (StatusCode::BAD_REQUEST, self.to_string()),
            Self::FileTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, self.to_string()),
            Self::AlreadyExists => (StatusCode::CONFLICT, self.to_string()),
            Self::Forbidden => (StatusCode::FORBIDDEN, self.to_string()),
            Self::Io(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()),
        };
        (status, msg).into_response()
    }
}

impl From<std::io::Error> for SamplesError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
