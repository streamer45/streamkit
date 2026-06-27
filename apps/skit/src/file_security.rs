// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::config::SecurityConfig;
use glob::Pattern;

/// Validates that a file path is safe for reading by file_read nodes.
/// This prevents directory traversal attacks and ensures paths are within allowed directories.
///
/// Relative paths are resolved against `asset_root` (the server's configured
/// `[server].asset_root`, which defaults to the process working directory), matching
/// how `core::file_reader` resolves paths at read time.
/// The resolved path must:
/// 1. Exist and be a regular file
/// 2. Be readable by the server process
/// 3. Be within configured allowed directories (from security.allowed_file_paths)
///
/// # Errors
///
/// Returns an error string if:
/// - The path cannot be canonicalized (missing/inaccessible file, or permission issues)
/// - The resolved path is outside `security.allowed_file_paths`
/// - The resolved path does not exist or is not a regular file
pub fn validate_file_path(
    path: &str,
    security_config: &SecurityConfig,
    asset_root: &std::path::Path,
) -> Result<(), String> {
    use std::path::{Path, PathBuf};

    let path_obj = Path::new(path);

    // Convert relative paths to absolute by joining with the asset root
    let absolute_path: PathBuf =
        if path_obj.is_absolute() { path_obj.to_path_buf() } else { asset_root.join(path_obj) };

    // Canonicalize path to resolve symlinks and ".." components
    // This is critical for security - prevents directory traversal
    let canonical_path = absolute_path.canonicalize().map_err(|e| {
        format!("Cannot resolve path '{path}' (file may not exist or is not accessible): {e}")
    })?;

    // Security: Check if path matches any allowed pattern
    let is_allowed =
        check_path_allowed(&canonical_path, asset_root, &security_config.allowed_file_paths);

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
/// - The path contains `..` components
/// - The parent directory cannot be canonicalized (missing/inaccessible dir)
/// - The resolved target path is outside `security.allowed_write_paths`
pub fn validate_write_path(
    path: &str,
    security_config: &SecurityConfig,
    asset_root: &std::path::Path,
) -> Result<(), String> {
    use std::path::{Component, Path, PathBuf};

    // Empty list means nothing is allowed (secure by default)
    if security_config.allowed_write_paths.is_empty() {
        return Err(
            "File writes are disabled by configuration (security.allowed_write_paths is empty)"
                .to_string(),
        );
    }

    let path_obj = Path::new(path);

    let absolute_path: PathBuf =
        if path_obj.is_absolute() { path_obj.to_path_buf() } else { asset_root.join(path_obj) };

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
        check_path_allowed(&canonical_target, asset_root, &security_config.allowed_write_paths);
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
/// - Relative patterns are resolved against `asset_root`
fn check_path_allowed(
    canonical_path: &std::path::Path,
    asset_root: &std::path::Path,
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
            // Make relative patterns absolute by prepending the asset root
            asset_root.join(pattern_str).to_string_lossy().to_string()
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

// `unwrap`/`expect` in tests: fixture setup failures should surface as panics at
// the failing operation rather than be propagated; this is the recommended test idiom.
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::SecurityConfig;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn read_config(patterns: &[&str]) -> SecurityConfig {
        SecurityConfig {
            allowed_file_paths: patterns.iter().map(|s| (*s).to_string()).collect(),
            allowed_write_paths: Vec::new(),
        }
    }

    fn write_config(patterns: &[&str]) -> SecurityConfig {
        SecurityConfig {
            allowed_file_paths: Vec::new(),
            allowed_write_paths: patterns.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    // TempDir lives on `/tmp` which is a real directory on this host, but canonicalize
    // anyway so tests stay correct on hosts where it isn't (e.g. macOS).
    fn canonical_tempdir() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("create tempdir");
        let canonical = dir.path().canonicalize().expect("canonicalize tempdir");
        (dir, canonical)
    }

    fn glob_under(dir: &Path) -> String {
        format!("{}/**", dir.display())
    }

    #[test]
    fn validate_file_path_accepts_file_inside_allowed_dir() {
        let (_tmp, root) = canonical_tempdir();
        let file = root.join("a.yml");
        fs::write(&file, b"x").unwrap();

        let cfg = read_config(&[&glob_under(&root)]);
        assert!(validate_file_path(file.to_str().unwrap(), &cfg, &root).is_ok());
    }

    #[test]
    fn validate_file_path_accepts_file_matched_by_glob() {
        let (_tmp, root) = canonical_tempdir();
        let nested = root.join("nested").join("deeply");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("a.yml");
        fs::write(&file, b"x").unwrap();

        let cfg = read_config(&[&glob_under(&root)]);
        assert!(validate_file_path(file.to_str().unwrap(), &cfg, &root).is_ok());
    }

    #[test]
    fn validate_file_path_rejects_missing_file() {
        let (_tmp, root) = canonical_tempdir();
        let missing = root.join("missing.yml");

        let cfg = read_config(&[&glob_under(&root)]);
        let err = validate_file_path(missing.to_str().unwrap(), &cfg, &root)
            .expect_err("missing file must be rejected");
        assert!(err.contains("Cannot resolve path"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_rejects_directory_target() {
        let (_tmp, root) = canonical_tempdir();
        let sub = root.join("subdir");
        fs::create_dir(&sub).unwrap();

        let cfg = read_config(&[&glob_under(&root)]);
        let err = validate_file_path(sub.to_str().unwrap(), &cfg, &root)
            .expect_err("directory must be rejected");
        assert!(err.contains("not a file"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_rejects_path_outside_allowlist() {
        let (_tmp_allowed, root_allowed) = canonical_tempdir();
        let (_tmp_other, root_other) = canonical_tempdir();
        let file = root_other.join("a.yml");
        fs::write(&file, b"x").unwrap();

        let cfg = read_config(&[&glob_under(&root_allowed)]);
        let err = validate_file_path(file.to_str().unwrap(), &cfg, &root_allowed)
            .expect_err("path outside allowlist must be rejected");
        assert!(err.contains("outside allowed directories"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_rejects_dot_dot_escape_to_sibling_dir() {
        let (_tmp, root) = canonical_tempdir();
        let allowed = root.join("allowed");
        let forbidden = root.join("forbidden");
        fs::create_dir_all(&allowed).unwrap();
        fs::create_dir_all(&forbidden).unwrap();
        fs::write(allowed.join("a.yml"), b"safe").unwrap();
        fs::write(forbidden.join("a.yml"), b"secret").unwrap();

        let cfg = read_config(&[&glob_under(&allowed)]);
        let traversal = format!("{}/../forbidden/a.yml", allowed.display());
        let err = validate_file_path(&traversal, &cfg, &allowed)
            .expect_err("`..` escape must be rejected");
        assert!(err.contains("outside allowed directories"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn validate_file_path_rejects_symlink_whose_target_is_outside_allowlist() {
        use std::os::unix::fs::symlink;

        let (_tmp, root) = canonical_tempdir();
        let allowed = root.join("allowed");
        let outside = root.join("outside");
        fs::create_dir_all(&allowed).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("secret.yml");
        fs::write(&target, b"secret").unwrap();
        let link = allowed.join("link.yml");
        symlink(&target, &link).unwrap();

        let cfg = read_config(&[&glob_under(&allowed)]);
        let err = validate_file_path(link.to_str().unwrap(), &cfg, &allowed)
            .expect_err("symlink escaping allowlist must be rejected");
        assert!(err.contains("outside allowed directories"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_empty_allowlist_denies_existing_file() {
        let (_tmp, root) = canonical_tempdir();
        let file = root.join("a.yml");
        fs::write(&file, b"x").unwrap();

        let cfg = read_config(&[]);
        let err = validate_file_path(file.to_str().unwrap(), &cfg, &root)
            .expect_err("empty allowlist must deny");
        assert!(err.contains("outside allowed directories"), "unexpected error: {err}");
    }

    #[test]
    fn validate_write_path_empty_allowlist_returns_disabled_message() {
        let cfg = write_config(&[]);
        let err = validate_write_path("/tmp/out.txt", &cfg, Path::new("/tmp"))
            .expect_err("empty write allowlist must disable writes");
        assert!(
            err.starts_with("File writes are disabled by configuration"),
            "unexpected error: {err}",
        );
    }

    #[test]
    fn validate_write_path_rejects_dot_dot_component() {
        let (_tmp, root) = canonical_tempdir();
        let cfg = write_config(&[&glob_under(&root)]);
        let target = format!("{}/sub/../escape.yml", root.display());

        let err = validate_write_path(&target, &cfg, &root)
            .expect_err("`..` in write path must be rejected");
        assert!(err.contains("must not contain '..' components"), "unexpected error: {err}");
    }

    #[test]
    fn validate_write_path_accepts_new_file_in_allowed_dir() {
        let (_tmp, root) = canonical_tempdir();
        let target = root.join("not_yet_created.yml");
        assert!(!target.exists(), "test precondition: target must not exist");

        let cfg = write_config(&[&glob_under(&root)]);
        assert!(validate_write_path(target.to_str().unwrap(), &cfg, &root).is_ok());
    }

    #[test]
    fn validate_write_path_rejects_when_parent_dir_missing() {
        let (_tmp, root) = canonical_tempdir();
        let target = root.join("does_not_exist").join("new.yml");

        let cfg = write_config(&[&glob_under(&root)]);
        let err = validate_write_path(target.to_str().unwrap(), &cfg, &root)
            .expect_err("missing parent dir must be rejected");
        assert!(err.contains("Cannot resolve parent directory"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn validate_write_path_rejects_when_symlinked_parent_escapes_allowlist() {
        use std::os::unix::fs::symlink;

        let (_tmp, root) = canonical_tempdir();
        let allowed = root.join("allowed");
        let outside = root.join("outside");
        fs::create_dir_all(&allowed).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let link = allowed.join("escape");
        symlink(&outside, &link).unwrap();

        let cfg = write_config(&[&glob_under(&allowed)]);
        let target = format!("{}/written.yml", link.display());
        let err = validate_write_path(&target, &cfg, &allowed)
            .expect_err("symlinked parent escaping allowlist must be rejected");
        assert!(err.contains("outside allowed write paths"), "unexpected error: {err}");
    }

    #[test]
    fn validate_write_path_requires_a_file_name() {
        let (_tmp, root) = canonical_tempdir();
        let cfg = write_config(&[&glob_under(&root)]);

        // `/` has no file name; this exercises the `file_name() == None` branch
        // without needing a `..` (which would trip the earlier ParentDir check).
        let err = validate_write_path("/", &cfg, &root).expect_err("root path has no file name");
        assert!(err.contains("must include a file name"), "unexpected error: {err}");
    }

    #[test]
    fn check_path_allowed_double_star_matches_everything() {
        let (_tmp, root) = canonical_tempdir();
        let file = root.join("a.yml");
        fs::write(&file, b"x").unwrap();
        let canonical = file.canonicalize().unwrap();

        let cwd = std::env::current_dir().unwrap();
        assert!(check_path_allowed(&canonical, &cwd, &[String::from("**")]));
    }

    #[test]
    fn check_path_allowed_glob_only_matches_under_named_subdir() {
        let (_tmp, root) = canonical_tempdir();
        let inside_dir = root.join("samples");
        let outside_dir = root.join("other");
        fs::create_dir_all(&inside_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        let inside_file = inside_dir.join("foo.yml");
        let outside_file = outside_dir.join("foo.yml");
        fs::write(&inside_file, b"x").unwrap();
        fs::write(&outside_file, b"x").unwrap();

        let pattern = glob_under(&inside_dir);
        let patterns = [pattern];
        let cwd = std::env::current_dir().unwrap();

        assert!(check_path_allowed(&inside_file.canonicalize().unwrap(), &cwd, &patterns));
        assert!(!check_path_allowed(&outside_file.canonicalize().unwrap(), &cwd, &patterns));
    }

    #[test]
    fn check_path_allowed_absolute_pattern_and_relative_resolves_against_cwd() {
        let (_tmp, root) = canonical_tempdir();
        let file = root.join("a.yml");
        fs::write(&file, b"x").unwrap();
        let canonical = file.canonicalize().unwrap();

        let abs_pattern = glob_under(&root);
        let real_cwd = std::env::current_dir().unwrap();
        assert!(check_path_allowed(&canonical, &real_cwd, &[abs_pattern]));

        // A relative pattern (`*.yml`) is joined with `cwd`; passing `root` as
        // the cwd parameter (without `set_current_dir`) makes the pattern resolve
        // to `<root>/*.yml` and match.
        assert!(check_path_allowed(&canonical, &root, &[String::from("*.yml")]));

        // A different cwd makes the same relative pattern miss.
        let (_other_tmp, other_root) = canonical_tempdir();
        assert!(!check_path_allowed(&canonical, &other_root, &[String::from("*.yml")]));
    }

    #[cfg(unix)]
    #[test]
    fn check_path_allowed_prefix_fallback_resolves_symlinked_pattern_dir() {
        use std::os::unix::fs::symlink;

        let (_tmp, root) = canonical_tempdir();
        let real_dir = root.join("real");
        fs::create_dir_all(&real_dir).unwrap();
        let link_dir = root.join("link");
        symlink(&real_dir, &link_dir).unwrap();
        let file = real_dir.join("a.yml");
        fs::write(&file, b"x").unwrap();
        let canonical = file.canonicalize().unwrap();

        // The glob `<link_dir>/**` won't literally match `<real_dir>/a.yml`,
        // but the prefix fallback canonicalizes `<link_dir>` → `<real_dir>` and
        // accepts the path. Without the fallback this would (incorrectly) fail.
        let pattern = glob_under(&link_dir);
        let cwd = std::env::current_dir().unwrap();
        assert!(check_path_allowed(&canonical, &cwd, &[pattern]));
    }

    // Regression for #521: relative paths and relative allow-list patterns must
    // resolve against the configured `asset_root`, not the process CWD, matching
    // how `core::file_reader` resolves paths at read time. A custom asset_root
    // (≠ CWD) must accept a relative path that lives under it.
    #[test]
    fn validate_file_path_resolves_relative_path_against_asset_root() {
        let (_tmp, asset_root) = canonical_tempdir();
        let file = asset_root.join("samples").join("a.yml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();

        let cfg = read_config(&[&glob_under(&asset_root)]);
        assert!(validate_file_path("samples/a.yml", &cfg, &asset_root).is_ok());

        // The same relative path resolved against a different asset_root misses.
        let (_other_tmp, other_root) = canonical_tempdir();
        assert!(validate_file_path("samples/a.yml", &cfg, &other_root).is_err());
    }

    #[test]
    fn validate_file_path_resolves_relative_pattern_against_asset_root() {
        let (_tmp, asset_root) = canonical_tempdir();
        let file = asset_root.join("samples").join("a.yml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();

        // A relative allow-list pattern is resolved against asset_root.
        let cfg = read_config(&["samples/**"]);
        assert!(validate_file_path(file.to_str().unwrap(), &cfg, &asset_root).is_ok());

        let (_other_tmp, other_root) = canonical_tempdir();
        assert!(validate_file_path(file.to_str().unwrap(), &cfg, &other_root).is_err());
    }

    #[test]
    fn validate_write_path_resolves_relative_path_against_asset_root() {
        let (_tmp, asset_root) = canonical_tempdir();
        let out_dir = asset_root.join("out");
        fs::create_dir_all(&out_dir).unwrap();

        let cfg = write_config(&[&glob_under(&asset_root)]);
        assert!(validate_write_path("out/new.yml", &cfg, &asset_root).is_ok());
    }

    // Regression for #521 follow-up: allow-list patterns without a trailing `/**`
    // are glob-matched (no prefix-canonicalize fallback), so they only match when
    // `asset_root` is already absolute + canonical. The server canonicalizes it
    // before threading it here; a relative root leaves the joined pattern relative
    // and it can never match the absolute canonical path.
    #[test]
    fn validate_file_path_matches_bare_pattern_against_canonical_asset_root() {
        let (_tmp, asset_root) = canonical_tempdir();
        let file = asset_root.join("samples").join("a.yml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();

        let cfg = read_config(&["samples/*.yml"]);
        assert!(validate_file_path("samples/a.yml", &cfg, &asset_root).is_ok());
    }

    #[test]
    fn check_path_allowed_needs_canonical_asset_root_for_bare_patterns() {
        let (_tmp, asset_root) = canonical_tempdir();
        let file = asset_root.join("samples").join("a.yml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();
        let canonical = file.canonicalize().unwrap();
        let patterns = ["samples/*.yml".to_string()];

        assert!(check_path_allowed(&canonical, &asset_root, &patterns));

        let relative_root = Path::new("some/relative/root");
        assert!(!check_path_allowed(&canonical, relative_root, &patterns));
    }
}
