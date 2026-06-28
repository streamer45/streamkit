// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use crate::config::SecurityConfig;
use glob::Pattern;

/// Bundles the file-security configuration with the `asset_root` it is always
/// used together with.
///
/// Validation must resolve paths in the same path-space the nodes read them
/// from, which means every check needs *both* the [`SecurityConfig`] and the
/// `asset_root`. Threading them as two adjacent parameters made it easy to pass
/// the config but forget `asset_root` — silently reintroducing the CWD/`asset_root`
/// mismatch that #521 fixed. Carrying them as one value built once at the call
/// boundary removes that footgun.
#[derive(Clone, Copy)]
pub struct FileSecurityPolicy<'a> {
    pub config: &'a SecurityConfig,
    pub asset_root: &'a std::path::Path,
}

impl<'a> FileSecurityPolicy<'a> {
    #[must_use]
    pub const fn new(config: &'a SecurityConfig, asset_root: &'a std::path::Path) -> Self {
        Self { config, asset_root }
    }
}

/// Validates that a file path is safe for reading by file_read nodes.
/// This prevents directory traversal attacks and ensures paths are within allowed directories.
///
/// Paths are **relative to `asset_root`** (the server's configured
/// `[server].asset_root`, which defaults to the process working directory):
/// absolute paths and `..` components are rejected. This is the same contract
/// the `core::file_reader`/`core::script` nodes enforce at read time (via
/// `path_helpers::resolve_existing_asset_path`), so a path that validates here
/// is exactly the path that is read.
/// The resolved path must:
/// 1. Be relative to `asset_root` (no absolute paths, no `..`)
/// 2. Exist, be a regular file, and stay within `asset_root` after symlink resolution
/// 3. Be within configured allowed directories (from security.allowed_file_paths)
///
/// # Errors
///
/// Returns an error string if:
/// - The path is absolute or contains `..`
/// - The path cannot be canonicalized (missing/inaccessible file, or permission issues)
/// - The resolved path escapes `asset_root` or is outside `security.allowed_file_paths`
/// - The resolved path is not a regular file
pub fn validate_file_path(path: &str, policy: FileSecurityPolicy<'_>) -> Result<(), String> {
    let canonical_path =
        streamkit_core::path_helpers::resolve_existing_asset_path(path, policy.asset_root)?;

    // Security: Check if path matches any allowed pattern
    let is_allowed =
        check_path_allowed(&canonical_path, policy.asset_root, &policy.config.allowed_file_paths);

    if !is_allowed {
        return Err(format!(
            "Path '{}' resolves to '{}' which is outside allowed directories. \
             Configure security.allowed_file_paths to allow additional paths.",
            path,
            canonical_path.display()
        ));
    }

    tracing::debug!("File path validation passed: '{}' -> '{}'", path, canonical_path.display());
    Ok(())
}

/// Validates that a file path is safe for writing by file_write nodes.
///
/// Like `validate_file_path`, the path is **relative to `asset_root`** and must
/// not be absolute or contain `..`. Unlike reads, the target may not exist yet,
/// so the parent directory is canonicalized (resolving symlinks) and the target
/// path is reconstructed beneath it — matching `core::file_writer`'s runtime
/// resolution (`path_helpers::resolve_new_asset_path`).
///
/// # Errors
///
/// Returns an error string if:
/// - The path is absolute or contains `..`
/// - The parent directory cannot be canonicalized (missing/inaccessible dir)
/// - The resolved target escapes `asset_root` or is outside `security.allowed_write_paths`
pub fn validate_write_path(path: &str, policy: FileSecurityPolicy<'_>) -> Result<(), String> {
    // Empty list means nothing is allowed (secure by default)
    if policy.config.allowed_write_paths.is_empty() {
        return Err(
            "File writes are disabled by configuration (security.allowed_write_paths is empty)"
                .to_string(),
        );
    }

    let canonical_target =
        streamkit_core::path_helpers::resolve_new_asset_path(path, policy.asset_root)?;

    let is_allowed = check_path_allowed(
        &canonical_target,
        policy.asset_root,
        &policy.config.allowed_write_paths,
    );
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
/// - Relative patterns are resolved against `asset_root` (whose glob
///   metacharacters are escaped so they match literally)
///
/// Since node paths are relative to `asset_root`, the matched `canonical_path`
/// always lives under `asset_root`; allow-list patterns further narrow which
/// files within it are permitted.
fn check_path_allowed(
    canonical_path: &std::path::Path,
    asset_root: &std::path::Path,
    allowed_patterns: &[String],
) -> bool {
    // Escape `asset_root`'s glob metacharacters once so a root like
    // `/srv/media[prod]` isn't parsed as a character class (which would silently
    // deny everything) when prepended to relative patterns.
    let escaped_root = Pattern::escape(&asset_root.to_string_lossy());

    for pattern_str in allowed_patterns {
        // Special case: "**" allows everything
        if pattern_str == "**" {
            return true;
        }

        let pattern_path = std::path::Path::new(pattern_str);

        // Build the glob string. For relative patterns we prepend the escaped
        // `asset_root`.
        let glob_str = if pattern_path.is_absolute() {
            pattern_str.clone()
        } else {
            format!("{escaped_root}/{pattern_str}")
        };

        match Pattern::new(&glob_str) {
            Ok(glob_pattern) if glob_pattern.matches_path(canonical_path) => return true,
            Ok(_) => {},
            Err(e) => tracing::warn!(
                pattern = %pattern_str,
                error = %e,
                "ignoring invalid allow-list glob pattern",
            ),
        }

        // Prefix fallback for directory patterns (e.g. `samples/**`): canonicalize
        // the literal directory (resolving symlinks) and accept paths beneath it.
        // Uses the unescaped literal path so it survives metacharacters in
        // `asset_root`.
        if let Some(stripped) = pattern_str.strip_suffix("/**") {
            let dir = if std::path::Path::new(stripped).is_absolute() {
                std::path::PathBuf::from(stripped)
            } else {
                asset_root.join(stripped)
            };
            if let Ok(prefix_canonical) = dir.canonicalize() {
                if canonical_path.starts_with(&prefix_canonical) {
                    return true;
                }
            }
        }
    }

    false
}

/// Test helper for a policy rooted at the current directory (`"."`).
///
/// This is the convention used by the validation/session/websocket
/// batch-operation tests; centralizing it keeps those call sites from
/// re-inlining the root choice.
#[cfg(test)]
pub fn cwd_policy(config: &SecurityConfig) -> FileSecurityPolicy<'_> {
    FileSecurityPolicy::new(config, std::path::Path::new("."))
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
        fs::write(root.join("a.yml"), b"x").unwrap();

        let cfg = read_config(&[&glob_under(&root)]);
        assert!(validate_file_path("a.yml", FileSecurityPolicy::new(&cfg, &root)).is_ok());
    }

    #[test]
    fn validate_file_path_accepts_file_matched_by_glob() {
        let (_tmp, root) = canonical_tempdir();
        let nested = root.join("nested").join("deeply");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("a.yml"), b"x").unwrap();

        let cfg = read_config(&[&glob_under(&root)]);
        assert!(
            validate_file_path("nested/deeply/a.yml", FileSecurityPolicy::new(&cfg, &root)).is_ok()
        );
    }

    #[test]
    fn validate_file_path_rejects_missing_file() {
        let (_tmp, root) = canonical_tempdir();

        let cfg = read_config(&[&glob_under(&root)]);
        let err = validate_file_path("missing.yml", FileSecurityPolicy::new(&cfg, &root))
            .expect_err("missing file must be rejected");
        assert!(err.contains("cannot resolve path"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_rejects_directory_target() {
        let (_tmp, root) = canonical_tempdir();
        fs::create_dir(root.join("subdir")).unwrap();

        let cfg = read_config(&[&glob_under(&root)]);
        let err = validate_file_path("subdir", FileSecurityPolicy::new(&cfg, &root))
            .expect_err("directory must be rejected");
        assert!(err.contains("not a regular file"), "unexpected error: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn validate_file_path_accepts_file_under_symlinked_asset_root() {
        use std::os::unix::fs::symlink;

        let (_tmp, root) = canonical_tempdir();
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("a.yml"), b"x").unwrap();
        let link = root.join("link");
        symlink(&real, &link).unwrap();

        // `link` is a non-canonical asset_root (a symlinked component). The
        // resolver must canonicalize it so the canonical file under `real` is
        // recognized as inside the root rather than false-rejected as outside.
        let cfg = read_config(&["**"]);
        assert!(
            validate_file_path("a.yml", FileSecurityPolicy::new(&cfg, &link)).is_ok(),
            "file under a symlinked asset_root must validate"
        );
    }

    #[test]
    fn validate_file_path_rejects_path_outside_allowlist() {
        let (_tmp, root) = canonical_tempdir();
        let other = root.join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("a.yml"), b"x").unwrap();

        // The allow-list only covers `<root>/allowed/**`; an existing file under a
        // sibling dir resolves within asset_root but is not allow-listed.
        let cfg = read_config(&[&glob_under(&root.join("allowed"))]);
        let err = validate_file_path("other/a.yml", FileSecurityPolicy::new(&cfg, &root))
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
        fs::write(forbidden.join("a.yml"), b"secret").unwrap();

        let cfg = read_config(&[&glob_under(&allowed)]);
        let err =
            validate_file_path("allowed/../forbidden/a.yml", FileSecurityPolicy::new(&cfg, &root))
                .expect_err("`..` escape must be rejected");
        assert!(err.contains("must not contain '..'"), "unexpected error: {err}");
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
        symlink(&target, allowed.join("link.yml")).unwrap();

        let cfg = read_config(&[&glob_under(&allowed)]);
        let err = validate_file_path("allowed/link.yml", FileSecurityPolicy::new(&cfg, &root))
            .expect_err("symlink escaping allowlist must be rejected");
        assert!(err.contains("outside allowed directories"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_empty_allowlist_denies_existing_file() {
        let (_tmp, root) = canonical_tempdir();
        fs::write(root.join("a.yml"), b"x").unwrap();

        let cfg = read_config(&[]);
        let err = validate_file_path("a.yml", FileSecurityPolicy::new(&cfg, &root))
            .expect_err("empty allowlist must deny");
        assert!(err.contains("outside allowed directories"), "unexpected error: {err}");
    }

    #[test]
    fn validate_file_path_rejects_absolute_path() {
        let (_tmp, root) = canonical_tempdir();
        let file = root.join("a.yml");
        fs::write(&file, b"x").unwrap();

        // The contract is relative-to-asset_root: absolute paths are rejected
        // outright, matching `core::file_reader`'s runtime `has_path_traversal`.
        let cfg = read_config(&[&glob_under(&root)]);
        let err = validate_file_path(file.to_str().unwrap(), FileSecurityPolicy::new(&cfg, &root))
            .expect_err("absolute path must be rejected");
        assert!(err.contains("must be relative to asset_root"), "unexpected error: {err}");
    }

    #[test]
    fn validate_write_path_empty_allowlist_returns_disabled_message() {
        let cfg = write_config(&[]);
        let err = validate_write_path("out.txt", FileSecurityPolicy::new(&cfg, Path::new("/tmp")))
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

        let err = validate_write_path("sub/../escape.yml", FileSecurityPolicy::new(&cfg, &root))
            .expect_err("`..` in write path must be rejected");
        assert!(err.contains("must not contain '..'"), "unexpected error: {err}");
    }

    #[test]
    fn validate_write_path_accepts_new_file_in_allowed_dir() {
        let (_tmp, root) = canonical_tempdir();
        assert!(
            !root.join("not_yet_created.yml").exists(),
            "test precondition: target must not exist"
        );

        let cfg = write_config(&[&glob_under(&root)]);
        assert!(validate_write_path("not_yet_created.yml", FileSecurityPolicy::new(&cfg, &root))
            .is_ok());
    }

    #[test]
    fn validate_write_path_rejects_when_parent_dir_missing() {
        let (_tmp, root) = canonical_tempdir();

        let cfg = write_config(&[&glob_under(&root)]);
        let err =
            validate_write_path("does_not_exist/new.yml", FileSecurityPolicy::new(&cfg, &root))
                .expect_err("missing parent dir must be rejected");
        assert!(err.contains("cannot resolve parent directory"), "unexpected error: {err}");
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
        symlink(&outside, allowed.join("escape")).unwrap();

        let cfg = write_config(&[&glob_under(&allowed)]);
        let err =
            validate_write_path("allowed/escape/written.yml", FileSecurityPolicy::new(&cfg, &root))
                .expect_err("symlinked parent escaping allowlist must be rejected");
        assert!(err.contains("outside allowed write paths"), "unexpected error: {err}");
    }

    #[test]
    fn validate_write_path_requires_a_file_name() {
        let (_tmp, root) = canonical_tempdir();
        let cfg = write_config(&[&glob_under(&root)]);

        // `.` resolves to a path with no file name, exercising the
        // `file_name() == None` branch without an absolute path or `..`.
        let err = validate_write_path(".", FileSecurityPolicy::new(&cfg, &root))
            .expect_err("path with no file name must be rejected");
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
        assert!(
            validate_file_path("samples/a.yml", FileSecurityPolicy::new(&cfg, &asset_root)).is_ok()
        );

        // The same relative path resolved against a different asset_root misses.
        let (_other_tmp, other_root) = canonical_tempdir();
        assert!(validate_file_path("samples/a.yml", FileSecurityPolicy::new(&cfg, &other_root))
            .is_err());
    }

    #[test]
    fn validate_file_path_resolves_relative_pattern_against_asset_root() {
        let (_tmp, asset_root) = canonical_tempdir();
        let file = asset_root.join("samples").join("a.yml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();

        // A relative allow-list pattern is resolved against asset_root.
        let cfg = read_config(&["samples/**"]);
        assert!(
            validate_file_path("samples/a.yml", FileSecurityPolicy::new(&cfg, &asset_root)).is_ok()
        );

        let (_other_tmp, other_root) = canonical_tempdir();
        assert!(validate_file_path("samples/a.yml", FileSecurityPolicy::new(&cfg, &other_root))
            .is_err());
    }

    #[test]
    fn validate_write_path_resolves_relative_path_against_asset_root() {
        let (_tmp, asset_root) = canonical_tempdir();
        let out_dir = asset_root.join("out");
        fs::create_dir_all(&out_dir).unwrap();

        let cfg = write_config(&[&glob_under(&asset_root)]);
        assert!(
            validate_write_path("out/new.yml", FileSecurityPolicy::new(&cfg, &asset_root)).is_ok()
        );
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
        assert!(
            validate_file_path("samples/a.yml", FileSecurityPolicy::new(&cfg, &asset_root)).is_ok()
        );
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

    // Regression for the glob-metachar finding: an `asset_root` containing glob
    // metacharacters (`[`, `]`, `*`, `?`) must be matched literally. Without
    // escaping the root prefix, a bare pattern like `samples/*.yml` joined under
    // `<root>/media[prod]` would parse `[prod]` as a character class and deny
    // every legitimately-allowed file.
    #[test]
    fn check_path_allowed_escapes_glob_metachars_in_asset_root() {
        let (_tmp, root) = canonical_tempdir();
        let asset_root = root.join("media[prod]");
        let file = asset_root.join("samples").join("a.yml");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"x").unwrap();
        let canonical = file.canonicalize().unwrap();

        let patterns = ["samples/*.yml".to_string()];
        assert!(check_path_allowed(&canonical, &asset_root, &patterns));
    }
}
