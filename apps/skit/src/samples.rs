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
    if filename.len() > MAX_FILENAME_LENGTH {
        return Err(SamplesError::InvalidFilename("Filename too long".to_string()));
    }

    if filename.is_empty() {
        return Err(SamplesError::InvalidFilename("Filename cannot be empty".to_string()));
    }

    if filename.contains("..") || filename.contains('/') || filename.contains('\\') {
        return Err(SamplesError::InvalidFilename("Invalid characters in filename".to_string()));
    }

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

/// Lists all available sample pipelines, filtered by permissions.
///
/// # Errors
///
/// Returns [`SamplesError::Io`] if the samples directory cannot be read.
pub async fn list_samples(
    app_state: &AppState,
    perms: &RolePermissions,
) -> Result<Vec<SamplePipeline>, SamplesError> {
    let base_path = PathBuf::from(&app_state.config.server.samples_dir);
    let mut samples = Vec::new();

    let oneshot_path = base_path.join("oneshot");
    if oneshot_path.exists() {
        samples.extend(load_samples_from_dir(&oneshot_path, true, "oneshot").await?);
    }

    let dynamic_path = base_path.join("dynamic");
    if dynamic_path.exists() {
        samples.extend(load_samples_from_dir(&dynamic_path, true, "dynamic").await?);
    }

    let user_path = base_path.join("user");
    if user_path.exists() {
        samples.extend(load_samples_from_dir(&user_path, false, "user").await?);
    }

    let demo_path = base_path.join("demo");
    if demo_path.exists() {
        samples.extend(load_samples_from_dir(&demo_path, true, "demo").await?);
    }

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

/// Metadata extracted from a pipeline YAML for listing, search, and discovery.
#[derive(Default)]
struct PipelineMetadata {
    name: Option<String>,
    description: Option<String>,
    mode: streamkit_api::EngineMode,
    explicit: crate::sample_discovery::Discovery,
    node_kinds: Vec<String>,
}

/// Parse pipeline YAML and extract metadata used for listing and discovery.
fn parse_pipeline_metadata(yaml: &str, path: &std::path::Path) -> PipelineMetadata {
    use streamkit_api::yaml::UserPipeline;

    let user_pipeline = match streamkit_api::yaml::parse_yaml(yaml) {
        Ok(p) => p,
        Err(e) => {
            warn!("Failed to parse pipeline metadata from {}: {}", path.display(), e);
            return PipelineMetadata::default();
        },
    };

    // The Steps and Dag arms differ only in how node kinds are collected, so
    // pull those out first and bind the shared discovery fields with one
    // irrefutable or-pattern.
    let node_kinds: Vec<String> = match &user_pipeline {
        UserPipeline::Steps { steps, .. } => steps.iter().map(|s| s.kind.clone()).collect(),
        UserPipeline::Dag { nodes, .. } => nodes.values().map(|n| n.kind.clone()).collect(),
    };

    let (UserPipeline::Steps {
        name,
        description,
        mode,
        group,
        variant,
        canonical,
        category,
        tags,
        keywords,
        ..
    }
    | UserPipeline::Dag {
        name,
        description,
        mode,
        group,
        variant,
        canonical,
        category,
        tags,
        keywords,
        ..
    }) = user_pipeline;

    PipelineMetadata {
        name,
        description,
        mode,
        explicit: crate::sample_discovery::Discovery {
            group,
            variant,
            canonical,
            category,
            tags,
            keywords,
        },
        node_kinds,
    }
}

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

        let metadata = fs::metadata(&path).await?;
        if metadata.len() > MAX_FILE_SIZE as u64 {
            warn!("Skipping file {} - exceeds size limit", path.display());
            continue;
        }

        match fs::read_to_string(&path).await {
            Ok(yaml) => {
                let meta = parse_pipeline_metadata(&yaml, &path);

                let base_filename = filename.trim_end_matches(".yml").trim_end_matches(".yaml");
                let id = format!("{subdir}/{base_filename}");

                let name = meta.name.unwrap_or_else(|| filename_to_name(filename));
                let description = meta.description.unwrap_or_default();
                let is_fragment = name == filename_to_name(filename) && description.is_empty();

                let search_terms = crate::sample_discovery::build_search_terms(
                    &name,
                    &description,
                    &meta.explicit,
                    &meta.node_kinds,
                );

                samples.push(SamplePipeline {
                    id,
                    name,
                    description,
                    yaml,
                    is_system,
                    mode: mode_to_string(meta.mode),
                    is_fragment,
                    group: meta.explicit.group,
                    variant: meta.explicit.variant,
                    canonical: meta.explicit.canonical,
                    category: meta.explicit.category,
                    tags: meta.explicit.tags,
                    search_terms,
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

/// Gets a specific sample pipeline by ID (e.g. `"oneshot/test-pipeline"`).
///
/// # Errors
///
/// Returns [`SamplesError::NotFound`] if the sample does not exist,
/// [`SamplesError::Forbidden`] if the caller lacks permission, or
/// [`SamplesError::Io`] on filesystem errors.
pub async fn get_sample(
    app_state: &AppState,
    id: &str,
    perms: &RolePermissions,
) -> Result<SamplePipeline, SamplesError> {
    let base_path = PathBuf::from(&app_state.config.server.samples_dir);

    let (subdir_hint, filename_base) = if let Some((prefix, base)) = id.split_once('/') {
        (Some(prefix), base)
    } else {
        (None, id)
    };

    // Reject path-traversal attempts in the filename portion
    if filename_base.is_empty()
        || filename_base.contains("..")
        || filename_base.contains('/')
        || filename_base.contains('\\')
    {
        return Err(SamplesError::InvalidFilename("Invalid characters in sample ID".to_string()));
    }

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

                let meta = parse_pipeline_metadata(&yaml, &path);
                let mode_str = mode_to_string(meta.mode);

                let name = meta.name.unwrap_or_else(|| filename_to_name(&filename));
                let description = meta.description.unwrap_or_default();

                let relative_path =
                    path.strip_prefix(&base_path).unwrap_or(&path).to_string_lossy().to_string();
                if !perms.is_sample_allowed(&relative_path) {
                    return Err(SamplesError::Forbidden);
                }

                let is_fragment = name == filename_to_name(&filename) && description.is_empty();
                let full_id = format!("{subdir}/{filename_base}");

                let search_terms = crate::sample_discovery::build_search_terms(
                    &name,
                    &description,
                    &meta.explicit,
                    &meta.node_kinds,
                );

                return Ok(SamplePipeline {
                    id: full_id,
                    name,
                    description,
                    yaml,
                    is_system,
                    mode: mode_str,
                    is_fragment,
                    group: meta.explicit.group,
                    variant: meta.explicit.variant,
                    canonical: meta.explicit.canonical,
                    category: meta.explicit.category,
                    tags: meta.explicit.tags,
                    search_terms,
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
    if !perms.write_samples {
        return SamplesError::Forbidden.into_response();
    }

    if request.yaml.len() > MAX_FILE_SIZE {
        return SamplesError::FileTooLarge.into_response();
    }

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

    fs::create_dir_all(&user_dir).await?;

    let path = user_dir.join(filename);

    if path.exists() && !request.overwrite {
        return Err(SamplesError::AlreadyExists);
    }

    let yaml_with_metadata = if request.is_fragment {
        request.yaml.clone()
    } else {
        match serde_saphyr::from_str::<serde_json::Value>(&request.yaml) {
            Ok(mut value) => {
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

    let meta = parse_pipeline_metadata(&yaml_with_metadata, &path);
    let mode_str = mode_to_string(meta.mode);

    let search_terms = crate::sample_discovery::build_search_terms(
        &request.name,
        &request.description,
        &meta.explicit,
        &meta.node_kinds,
    );

    Ok(SamplePipeline {
        id,
        name: request.name.clone(),
        description: request.description.clone(),
        yaml: request.yaml.clone(),
        is_system: false,
        mode: mode_str,
        is_fragment: request.is_fragment,
        group: meta.explicit.group,
        variant: meta.explicit.variant,
        canonical: meta.explicit.canonical,
        category: meta.explicit.category,
        tags: meta.explicit.tags,
        search_terms,
    })
}

/// Deletes a user pipeline
async fn delete_sample_handler(
    State(app_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let perms = get_permissions(&headers, &app_state);
    if !perms.delete_samples {
        warn!(
            sample_id = %id,
            delete_samples = perms.delete_samples,
            "Blocked attempt to delete sample: permission denied"
        );
        return SamplesError::Forbidden.into_response();
    }

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

    let (subdir_hint, filename_base) = if let Some((prefix, base)) = id.split_once('/') {
        (Some(prefix), base)
    } else {
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
pub enum SamplesError {
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

#[cfg(test)]
// reason: tests use expect/unwrap-style helpers so a failed assertion produces
// a single clear panic instead of nested Result-handling boilerplate.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use streamkit_api::EngineMode;

    fn invalid_filename_msg(err: &SamplesError) -> &str {
        match err {
            SamplesError::InvalidFilename(msg) => msg.as_str(),
            other => panic!("expected InvalidFilename, got {other:?}"),
        }
    }

    #[test]
    fn validate_filename_accepts_yml_yaml_and_uppercase() {
        for ok in ["foo.yml", "foo.yaml", "FOO.YML", "FOO.YAML", "Foo.Yml", "mixed.YaMl"] {
            assert!(validate_filename(ok).is_ok(), "expected `{ok}` to be accepted");
        }
    }

    #[test]
    fn validate_filename_rejects_empty() {
        let err = validate_filename("").expect_err("empty filename must be rejected");
        assert!(matches!(err, SamplesError::InvalidFilename(_)));
        assert!(invalid_filename_msg(&err).to_ascii_lowercase().contains("empty"));
    }

    #[test]
    fn validate_filename_rejects_too_long() {
        // 252 alphanumeric + ".yml" (4) = 256 chars total, which is > 255 (MAX_FILENAME_LENGTH)
        let too_long = format!("{}.yml", "a".repeat(252));
        assert_eq!(too_long.len(), MAX_FILENAME_LENGTH + 1);

        let err = validate_filename(&too_long).expect_err("over-length filename must be rejected");
        assert!(matches!(err, SamplesError::InvalidFilename(_)));
        assert!(invalid_filename_msg(&err).to_ascii_lowercase().contains("long"));
    }

    #[test]
    fn validate_filename_accepts_max_length() {
        // 251 alphanumeric + ".yml" (4) = 255 chars total == MAX_FILENAME_LENGTH (boundary).
        let at_limit = format!("{}.yml", "a".repeat(251));
        assert_eq!(at_limit.len(), MAX_FILENAME_LENGTH);
        assert!(validate_filename(&at_limit).is_ok());
    }

    #[test]
    fn validate_filename_rejects_path_traversal_and_separators() {
        for bad in ["../foo.yml", "foo..yml", "..yml.yml", "foo/bar.yml", "foo\\bar.yml"] {
            match validate_filename(bad) {
                Err(SamplesError::InvalidFilename(_)) => {},
                Ok(()) => panic!("expected `{bad}` to be rejected"),
                Err(other) => panic!("expected InvalidFilename for `{bad}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn validate_filename_rejects_missing_or_wrong_extension() {
        for bad in ["foo", "foo.", "foo.txt", "foo.json", "yaml"] {
            match validate_filename(bad) {
                Err(SamplesError::InvalidFilename(_)) => {},
                Ok(()) => panic!("expected `{bad}` to be rejected as missing/wrong extension"),
                Err(other) => panic!("expected InvalidFilename for `{bad}`, got {other:?}"),
            }
        }
    }

    #[test]
    fn filename_to_name_strips_extension_and_capitalises() {
        assert_eq!(filename_to_name("hello_world.yml"), "Hello World");
        assert_eq!(filename_to_name("single.yaml"), "Single");
        assert_eq!(filename_to_name("multi_word_pipeline.yml"), "Multi Word Pipeline");
    }

    #[test]
    fn filename_to_name_without_extension_still_capitalises() {
        assert_eq!(filename_to_name("bare"), "Bare");
        assert_eq!(filename_to_name("two_words"), "Two Words");
    }

    #[test]
    fn filename_to_name_empty_input_returns_empty() {
        assert_eq!(filename_to_name(""), "");
    }

    #[test]
    fn filename_to_name_collapses_internal_whitespace() {
        assert_eq!(filename_to_name("a__b.yml"), "A B");
    }

    #[test]
    fn generate_safe_filename_slugifies_to_underscore_lowercase_yml() {
        assert_eq!(generate_safe_filename("My Pipeline"), "my_pipeline.yml");
        assert_eq!(generate_safe_filename("HELLO"), "hello.yml");
        assert_eq!(generate_safe_filename("keep-dashes"), "keep-dashes.yml");
        assert_eq!(generate_safe_filename("snake_case"), "snake_case.yml");
    }

    #[test]
    fn generate_safe_filename_strips_disallowed_characters() {
        assert_eq!(generate_safe_filename("a/b\\c"), "a_b_c.yml");
        assert_eq!(generate_safe_filename("dots..everywhere"), "dots__everywhere.yml");
        assert_eq!(generate_safe_filename("weird!@#name"), "weird___name.yml");
    }

    #[test]
    fn generate_safe_filename_trims_leading_and_trailing_underscores() {
        assert_eq!(generate_safe_filename("/etc/passwd"), "etc_passwd.yml");
        assert_eq!(generate_safe_filename("...trailing..."), "trailing.yml");
    }

    #[test]
    fn generate_safe_filename_empty_uses_timestamp_fallback() {
        // Either an empty string or one that becomes empty after sanitisation
        // must produce a deterministic, validator-passing fallback.
        for input in ["", "/", "..", "___", "!!!"] {
            let result = generate_safe_filename(input);
            let ends_with_yml = std::path::Path::new(&result)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("yml"));
            assert!(
                result.starts_with("pipeline_") && ends_with_yml,
                "input `{input}` produced unexpected fallback `{result}`"
            );
            // The numeric portion must be all digits (the unix timestamp).
            let middle = result
                .strip_prefix("pipeline_")
                .and_then(|s| s.strip_suffix(".yml"))
                .expect("fallback must have the documented shape");
            assert!(
                !middle.is_empty() && middle.chars().all(|c| c.is_ascii_digit()),
                "fallback timestamp portion of `{result}` was not numeric"
            );
        }
    }

    #[test]
    fn generate_safe_filename_round_trips_through_validate() {
        // Property: every output of generate_safe_filename must be accepted by
        // validate_filename, including for hostile inputs.
        for input in [
            "ok",
            "My Pipeline",
            "a/b/c",
            "..\\..\\etc\\passwd",
            "weird!@#name",
            "",
            "...",
            "___",
            "trailing///",
        ] {
            let generated = generate_safe_filename(input);
            assert!(
                validate_filename(&generated).is_ok(),
                "input `{input}` produced `{generated}` which validate_filename rejected"
            );
        }
    }

    #[test]
    fn has_yaml_extension_accepts_yml_and_yaml_case_insensitive() {
        for ok in ["foo.yml", "foo.yaml", "foo.YML", "foo.YAML", "foo.YmL"] {
            assert!(has_yaml_extension(ok), "expected `{ok}` to be recognised");
        }
    }

    #[test]
    fn has_yaml_extension_rejects_other_or_missing_extensions() {
        for bad in ["foo.txt", "foo", "foo.json", "foo.yml.gz", "yaml", ""] {
            assert!(!has_yaml_extension(bad), "expected `{bad}` to be rejected");
        }
    }

    #[test]
    fn mode_to_string_maps_every_variant() {
        // Exhaustive match guards against future drift: a new variant will
        // fail to compile until this table is updated.
        for mode in [EngineMode::OneShot, EngineMode::Dynamic] {
            let expected = match mode {
                EngineMode::OneShot => "oneshot",
                EngineMode::Dynamic => "dynamic",
            };
            assert_eq!(mode_to_string(mode), expected);
        }
    }

    #[test]
    fn parse_pipeline_metadata_extracts_name_description_and_mode() {
        let yaml = "
name: \"My Sample\"
description: \"A nice description\"
mode: oneshot
nodes:
  input:
    kind: streamkit::http_input
  output:
    kind: streamkit::http_output
    needs: input
";
        let path = std::path::Path::new("test.yml");
        let meta = parse_pipeline_metadata(yaml, path);

        assert_eq!(meta.name.as_deref(), Some("My Sample"));
        assert_eq!(meta.description.as_deref(), Some("A nice description"));
        assert_eq!(meta.mode, EngineMode::OneShot);
    }

    #[test]
    fn parse_pipeline_metadata_works_for_steps_form() {
        let yaml = "
name: \"Linear\"
description: \"Steps form\"
mode: dynamic
steps:
  - kind: streamkit::http_input
  - kind: streamkit::http_output
";
        let path = std::path::Path::new("steps.yml");
        let meta = parse_pipeline_metadata(yaml, path);

        assert_eq!(meta.name.as_deref(), Some("Linear"));
        assert_eq!(meta.description.as_deref(), Some("Steps form"));
        assert_eq!(meta.mode, EngineMode::Dynamic);
        assert_eq!(meta.node_kinds, vec!["streamkit::http_input", "streamkit::http_output"]);
    }

    #[test]
    fn parse_pipeline_metadata_missing_optional_fields_returns_none() {
        // No name, no description, no explicit mode -- mode must default
        // (not error) and the name/description options must be None.
        let yaml = "
nodes:
  input:
    kind: streamkit::http_input
";
        let path = std::path::Path::new("anon.yml");
        let meta = parse_pipeline_metadata(yaml, path);

        assert!(meta.name.is_none(), "expected no name, got {:?}", meta.name);
        assert!(meta.description.is_none(), "expected no description, got {:?}", meta.description);
        assert_eq!(meta.mode, EngineMode::default());
        assert_eq!(meta.mode, EngineMode::Dynamic, "Dynamic must be the documented default");
    }

    #[test]
    fn parse_pipeline_metadata_mode_defaults_when_omitted() {
        let yaml = "
name: \"No mode\"
nodes:
  input:
    kind: streamkit::http_input
";
        let path = std::path::Path::new("no-mode.yml");
        let meta = parse_pipeline_metadata(yaml, path);

        assert_eq!(meta.name.as_deref(), Some("No mode"));
        assert_eq!(meta.mode, EngineMode::default());
    }

    #[test]
    fn parse_pipeline_metadata_returns_defaults_on_malformed_yaml() {
        // Garbage that is neither a Steps nor a Dag pipeline. The helper
        // is intentionally lenient -- it logs and returns defaults rather
        // than propagating an error, since the listing code uses it for
        // best-effort enrichment.
        let yaml = "this: is: not: valid: yaml: at: all: [";
        let path = std::path::Path::new("broken.yml");
        let meta = parse_pipeline_metadata(yaml, path);

        assert!(meta.name.is_none());
        assert!(meta.description.is_none());
        assert_eq!(meta.mode, EngineMode::default());
    }
}

#[cfg(test)]
// reason: handler tests intentionally use unwrap/expect so setup failures and
// response decoding failures produce direct assertion panics.
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod handler_tests {
    use super::*;
    use crate::config::{AuthMode, Config};
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use streamkit_api::{SamplePipeline, SavePipelineRequest};
    use tempfile::TempDir;
    use tower::ServiceExt;

    fn make_state(samples_dir: &std::path::Path) -> Arc<AppState> {
        let mut config = Config::default();
        config.auth.mode = AuthMode::Disabled;
        config.server.samples_dir = samples_dir.to_string_lossy().into_owned();
        config
            .permissions
            .roles
            .insert("admin".to_string(), crate::permissions::Permissions::admin());
        crate::server::create_app_state(config, None)
    }

    /// Build a fixture samples tree.
    fn make_samples_tree() -> TempDir {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path();

        std::fs::create_dir_all(base.join("oneshot")).unwrap();
        std::fs::create_dir_all(base.join("dynamic")).unwrap();
        std::fs::create_dir_all(base.join("user")).unwrap();

        std::fs::write(
            base.join("oneshot").join("alpha.yml"),
            "name: \"Alpha\"\ndescription: \"alpha desc\"\nmode: oneshot\nnodes:\n  in:\n    kind: streamkit::http_input\n",
        )
        .unwrap();

        std::fs::write(
            base.join("dynamic").join("beta.yaml"),
            "name: \"Beta\"\ndescription: \"beta desc\"\nmode: dynamic\nnodes:\n  in:\n    kind: streamkit::http_input\n",
        )
        .unwrap();

        // Hidden file and non-yaml file must be filtered by has_yaml_extension.
        std::fs::write(base.join("oneshot").join(".DS_Store"), b"junk").unwrap();
        std::fs::write(base.join("oneshot").join("notes.txt"), b"ignored").unwrap();
        // Dotdir contents would only be touched if the loader recursed, which it
        // intentionally does not.
        std::fs::create_dir_all(base.join("oneshot").join(".hidden")).unwrap();
        std::fs::write(base.join("oneshot").join(".hidden").join("snuck.yml"), "name: \"snuck\"\n")
            .unwrap();

        tmp
    }

    #[tokio::test]
    async fn list_samples_returns_oneshot_and_dynamic_and_filters_non_yaml() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());
        let perms = crate::permissions::Permissions::admin();

        let samples = list_samples(&state, &perms).await.unwrap();

        let ids: Vec<_> = samples.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"oneshot/alpha"), "missing oneshot sample, got {ids:?}");
        assert!(ids.contains(&"dynamic/beta"), "missing dynamic sample, got {ids:?}");

        // .DS_Store, .txt files, and dotdir contents must not leak into the list.
        for s in &samples {
            assert!(!s.id.ends_with(".DS_Store"), "hidden file leaked: {}", s.id);
            assert!(!s.id.ends_with("notes"), "non-yaml file leaked: {}", s.id);
            assert!(!s.id.contains(".hidden"), "dotdir traversal leaked: {}", s.id);
        }

        let alpha = samples.iter().find(|s| s.id == "oneshot/alpha").unwrap();
        assert_eq!(alpha.name, "Alpha");
        assert_eq!(alpha.description, "alpha desc");
        assert_eq!(alpha.mode, "oneshot");
        assert!(alpha.is_system, "bundled samples must be marked as system");
    }

    #[tokio::test]
    async fn list_samples_missing_dir_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        // No subdirs created -- every base/<subdir>.exists() check returns false.
        let state = make_state(tmp.path());
        let perms = crate::permissions::Permissions::admin();

        let samples = list_samples(&state, &perms).await.unwrap();
        assert!(samples.is_empty(), "missing dir must yield Ok(vec![]), got {samples:?}");
    }

    #[tokio::test]
    async fn list_samples_handler_filters_to_oneshot_only() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());

        let app = samples_router().with_state(state);
        let resp = app
            .clone()
            .oneshot(Request::builder().uri("/api/v1/samples/oneshot").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let samples: Vec<SamplePipeline> = serde_json::from_slice(&body).unwrap();
        assert!(samples.iter().all(|s| s.mode == "oneshot"));
        assert!(samples.iter().any(|s| s.id == "oneshot/alpha"));
    }

    #[tokio::test]
    async fn list_dynamic_samples_handler_filters_to_dynamic_only() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());

        let app = samples_router().with_state(state);
        let resp = app
            .oneshot(Request::builder().uri("/api/v1/samples/dynamic").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
        let samples: Vec<SamplePipeline> = serde_json::from_slice(&body).unwrap();
        assert!(samples.iter().all(|s| s.mode == "dynamic"));
        assert!(samples.iter().any(|s| s.id == "dynamic/beta"));
    }

    #[tokio::test]
    async fn get_sample_returns_existing_oneshot_with_namespaced_id() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());
        let perms = crate::permissions::Permissions::admin();

        let sample = get_sample(&state, "oneshot/alpha", &perms).await.unwrap();
        assert_eq!(sample.id, "oneshot/alpha");
        assert_eq!(sample.name, "Alpha");
        assert_eq!(sample.mode, "oneshot");
        assert!(sample.yaml.contains("streamkit::http_input"));
    }

    #[tokio::test]
    async fn get_sample_unknown_id_returns_not_found() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());
        let perms = crate::permissions::Permissions::admin();

        let err = get_sample(&state, "oneshot/does-not-exist", &perms).await.unwrap_err();
        assert!(matches!(err, SamplesError::NotFound), "expected NotFound, got {err:?}");
    }

    #[tokio::test]
    async fn get_sample_path_traversal_in_id_is_rejected() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());
        let perms = crate::permissions::Permissions::admin();

        // The legacy id form (no prefix) carrying a `..` segment must be rejected
        // by `get_sample` before any filesystem access.
        let err = get_sample(&state, "../../../etc/passwd", &perms).await.unwrap_err();
        assert!(
            matches!(err, SamplesError::InvalidFilename(_)),
            "expected InvalidFilename, got {err:?}"
        );
    }

    #[tokio::test]
    async fn get_sample_handler_rejects_path_traversal_with_400() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());
        let app = samples_router().with_state(state);

        // axum path matching consumes the entire `{id}` segment, so we URL-encode
        // the slashes to keep the traversal attempt embedded in the id.
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/v1/samples/oneshot/..%2F..%2Fetc%2Fpasswd")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "path-traversal request must produce 400, got {:?}",
            resp.status()
        );
    }

    #[tokio::test]
    async fn save_sample_writes_user_dir_and_round_trips() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(tmp.path());
        let perms = crate::permissions::Permissions::admin();

        let request = SavePipelineRequest {
            name: "My Pipeline".to_string(),
            description: "description here".to_string(),
            yaml: "mode: oneshot\nnodes:\n  in:\n    kind: streamkit::http_input\n".to_string(),
            overwrite: false,
            is_fragment: false,
        };
        let filename = generate_safe_filename(&request.name);
        assert_eq!(filename, "my_pipeline.yml");

        let saved = save_sample(&state, &filename, &request).await.unwrap();
        assert_eq!(saved.id, "user/my_pipeline");
        assert!(!saved.is_system);

        // Verify the file actually exists on disk.
        let user_path = tmp.path().join("user").join("my_pipeline.yml");
        assert!(user_path.exists(), "user pipeline must be persisted at {}", user_path.display());

        // Round-trip: get_sample must find the just-saved entry.
        let fetched = get_sample(&state, "user/my_pipeline", &perms).await.unwrap();
        assert_eq!(fetched.id, "user/my_pipeline");
        assert!(!fetched.is_system);
    }

    #[tokio::test]
    async fn save_sample_without_overwrite_reports_conflict() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(tmp.path());

        let req = SavePipelineRequest {
            name: "dup".to_string(),
            description: String::new(),
            yaml: "mode: dynamic\n".to_string(),
            overwrite: false,
            is_fragment: true,
        };
        save_sample(&state, "dup.yml", &req).await.unwrap();

        let err = save_sample(&state, "dup.yml", &req).await.unwrap_err();
        assert!(
            matches!(err, SamplesError::AlreadyExists),
            "expected AlreadyExists on second write without overwrite, got {err:?}"
        );
    }

    #[tokio::test]
    async fn save_sample_with_overwrite_replaces_existing() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(tmp.path());

        let req = SavePipelineRequest {
            name: "overw".to_string(),
            description: "first".to_string(),
            yaml: "mode: dynamic\nnodes: {}\n".to_string(),
            overwrite: false,
            is_fragment: false,
        };
        save_sample(&state, "overw.yml", &req).await.unwrap();

        let mut req2 = req.clone();
        req2.description = "second".to_string();
        req2.overwrite = true;
        let result = save_sample(&state, "overw.yml", &req2).await.unwrap();
        assert_eq!(result.description, "second");
    }

    #[tokio::test]
    async fn delete_sample_removes_user_file_and_returns_not_found_after() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(tmp.path());

        let req = SavePipelineRequest {
            name: "to-delete".to_string(),
            description: String::new(),
            yaml: "mode: dynamic\n".to_string(),
            overwrite: false,
            is_fragment: true,
        };
        save_sample(&state, "to-delete.yml", &req).await.unwrap();
        assert!(tmp.path().join("user").join("to-delete.yml").exists());

        delete_sample(&state, "user/to-delete").await.unwrap();
        assert!(!tmp.path().join("user").join("to-delete.yml").exists());

        let err = delete_sample(&state, "user/to-delete").await.unwrap_err();
        assert!(matches!(err, SamplesError::NotFound));
    }

    #[tokio::test]
    async fn delete_sample_refuses_to_touch_system_dirs() {
        let tmp = make_samples_tree();
        let state = make_state(tmp.path());

        // Bundled oneshot/* samples must never be deletable via this code path.
        let err = delete_sample(&state, "oneshot/alpha").await.unwrap_err();
        assert!(matches!(err, SamplesError::Forbidden), "expected Forbidden, got {err:?}");
        // The bundled file must still be present after the rejected delete.
        assert!(tmp.path().join("oneshot").join("alpha.yml").exists());
    }

    #[tokio::test]
    async fn save_sample_handler_rejects_oversize_yaml_with_413() {
        let tmp = TempDir::new().unwrap();
        let state = make_state(tmp.path());
        let app = samples_router().with_state(state);

        let oversize = "a: 1\n".repeat(MAX_FILE_SIZE / 4 + 4);
        let req = SavePipelineRequest {
            name: "too_big".to_string(),
            description: String::new(),
            yaml: oversize,
            overwrite: false,
            is_fragment: true,
        };
        let body = serde_json::to_vec(&req).unwrap();
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/samples/oneshot")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn samples_error_status_codes_cover_every_variant() {
        let cases: Vec<(SamplesError, StatusCode)> = vec![
            (SamplesError::NotFound, StatusCode::NOT_FOUND),
            (SamplesError::InvalidFilename("x".into()), StatusCode::BAD_REQUEST),
            (SamplesError::FileTooLarge, StatusCode::PAYLOAD_TOO_LARGE),
            (SamplesError::AlreadyExists, StatusCode::CONFLICT),
            (SamplesError::Forbidden, StatusCode::FORBIDDEN),
            (SamplesError::Io(std::io::Error::other("boom")), StatusCode::INTERNAL_SERVER_ERROR),
        ];
        for (err, expected) in cases {
            assert_eq!(err.into_response().status(), expected);
        }
    }
}
