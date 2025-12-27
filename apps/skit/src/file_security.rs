// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::config::SecurityConfig;
use glob::Pattern;

/// Validates that a file path is safe for reading by file_read nodes.
/// This prevents directory traversal attacks and ensures paths are within allowed directories.
///
/// Relative paths are resolved against the server's current working directory.
/// The resolved path must:
/// 1. Exist and be a regular file
/// 2. Be readable by the server process
/// 3. Be within configured allowed directories (from security.allowed_file_paths)
///
/// # Errors
///
/// Returns an error string if:
/// - The current working directory cannot be determined
/// - The path cannot be canonicalized (missing/inaccessible file, or permission issues)
/// - The resolved path is outside `security.allowed_file_paths`
/// - The resolved path does not exist or is not a regular file
pub fn validate_file_path(path: &str, security_config: &SecurityConfig) -> Result<(), String> {
    use std::path::{Path, PathBuf};

    let path_obj = Path::new(path);

    // Get current working directory for resolving relative paths
    let cwd = std::env::current_dir()
        .map_err(|e| format!("Failed to get current working directory: {e}"))?;

    // Convert relative paths to absolute by joining with current working directory
    let absolute_path: PathBuf =
        if path_obj.is_absolute() { path_obj.to_path_buf() } else { cwd.join(path_obj) };

    // Canonicalize path to resolve symlinks and ".." components
    // This is critical for security - prevents directory traversal
    let canonical_path = absolute_path.canonicalize().map_err(|e| {
        format!("Cannot resolve path '{path}' (file may not exist or is not accessible): {e}")
    })?;

    // Security: Check if path matches any allowed pattern
    let is_allowed = check_path_allowed(&canonical_path, &cwd, &security_config.allowed_file_paths);

    if !is_allowed {
        return Err(format!(
            "Path '{}' resolves to '{}' which is outside allowed directories. \
             Configure security.allowed_file_paths to allow additional paths.",
            path,
            canonical_path.display()
        ));
    }

    // Verify file exists and is readable
    if !canonical_path.exists() {
        return Err(format!(
            "File does not exist: '{}' (resolved from '{}')",
            canonical_path.display(),
            path
        ));
    }

    if !canonical_path.is_file() {
        return Err(format!(
            "Path is not a file: '{}' (resolved from '{}')",
            canonical_path.display(),
            path
        ));
    }

    tracing::debug!("File path validation passed: '{}' -> '{}'", path, canonical_path.display());
    Ok(())
}

/// Validates that a file path is safe for writing by file_write nodes.
///
/// Unlike `validate_file_path`, the target may not exist yet. We validate the parent directory
/// by canonicalizing it (resolving symlinks) and then reconstructing the target path.
///
/// # Errors
///
/// Returns an error string if:
/// - The current working directory cannot be determined
/// - The path contains `..` components
/// - The parent directory cannot be canonicalized (missing/inaccessible dir)
/// - The resolved target path is outside `security.allowed_write_paths`
pub fn validate_write_path(path: &str, security_config: &SecurityConfig) -> Result<(), String> {
    use std::path::{Component, Path, PathBuf};

    // Empty list means nothing is allowed (secure by default)
    if security_config.allowed_write_paths.is_empty() {
        return Err(
            "File writes are disabled by configuration (security.allowed_write_paths is empty)"
                .to_string(),
        );
    }

    let path_obj = Path::new(path);

    let cwd = std::env::current_dir()
        .map_err(|e| format!("Failed to get current working directory: {e}"))?;

    let absolute_path: PathBuf =
        if path_obj.is_absolute() { path_obj.to_path_buf() } else { cwd.join(path_obj) };

    // Reject parent-dir traversal for writes (canonicalize may not be possible if file doesn't exist).
    if absolute_path.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("Write path must not contain '..' components: '{path}'"));
    }

    let file_name = absolute_path
        .file_name()
        .ok_or_else(|| format!("Write path must include a file name: '{path}'"))?
        .to_owned();

    let parent = absolute_path
        .parent()
        .ok_or_else(|| format!("Write path must have a parent directory: '{path}'"))?;

    let canonical_parent = parent.canonicalize().map_err(|e| {
        format!(
            "Cannot resolve parent directory '{}' for write path '{}': {e}",
            parent.display(),
            path
        )
    })?;

    let canonical_target = canonical_parent.join(file_name);

    let is_allowed =
        check_path_allowed(&canonical_target, &cwd, &security_config.allowed_write_paths);
    if !is_allowed {
        return Err(format!(
            "Write path '{}' resolves to '{}' which is outside allowed write paths. \
             Configure security.allowed_write_paths to allow additional paths.",
            path,
            canonical_target.display()
        ));
    }

    tracing::debug!("Write path validation passed: '{}' -> '{}'", path, canonical_target.display());
    Ok(())
}

/// Check if a canonical path is allowed by the configured patterns.
///
/// Patterns can be:
/// - `**` - Allow all paths (not recommended for production)
/// - `samples/**` - Allow all files under the samples directory
/// - `/absolute/path/**` - Allow all files under an absolute path
/// - Relative patterns are resolved against the current working directory
fn check_path_allowed(
    canonical_path: &std::path::Path,
    cwd: &std::path::Path,
    allowed_patterns: &[String],
) -> bool {
    for pattern_str in allowed_patterns {
        // Special case: "**" allows everything
        if pattern_str == "**" {
            return true;
        }

        // Resolve pattern to absolute path if it's relative
        let pattern_path = std::path::Path::new(pattern_str);
        let absolute_pattern = if pattern_path.is_absolute() {
            pattern_str.clone()
        } else {
            // Make relative patterns absolute by prepending cwd
            cwd.join(pattern_str).to_string_lossy().to_string()
        };

        // Try to match using glob pattern
        if let Ok(glob_pattern) = Pattern::new(&absolute_pattern) {
            if glob_pattern.matches_path(canonical_path) {
                return true;
            }
        }

        // Also try prefix matching for directory patterns (e.g., "samples/**" -> starts with "samples/")
        if absolute_pattern.ends_with("/**") {
            let prefix = &absolute_pattern[..absolute_pattern.len() - 3]; // Remove "/**"
            if let Ok(prefix_canonical) = std::path::Path::new(prefix).canonicalize() {
                if canonical_path.starts_with(&prefix_canonical) {
                    return true;
                }
            }
        }
    }

    false
}
