// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, Context, Result};

/// Validates a single path component used in plugin storage paths.
///
/// # Errors
///
/// Returns an error if the component is empty, contains whitespace, or is not a single path
/// segment.
pub fn validate_path_component(label: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(anyhow!("{label} must not be empty"));
    }
    if value != value.trim() {
        return Err(anyhow!("{label} must not contain leading or trailing whitespace"));
    }

    let mut components = Path::new(value).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(()),
        _ => Err(anyhow!("{label} must be a single path component")),
    }
}

/// Ensures the plugin base directory exists and returns its canonical path.
///
/// # Errors
///
/// Returns an error if the directory cannot be created or canonicalized.
pub async fn ensure_base_dir(base_dir: &Path) -> Result<PathBuf> {
    tokio::fs::create_dir_all(base_dir).await.with_context(|| {
        format!("Failed to create plugin base dir {base_dir}", base_dir = base_dir.display())
    })?;
    tokio::fs::canonicalize(base_dir).await.with_context(|| {
        format!("Failed to resolve plugin base dir {base_dir}", base_dir = base_dir.display())
    })
}

/// Canonicalizes an existing plugin base directory.
///
/// # Errors
///
/// Returns an error if the directory cannot be canonicalized.
pub async fn canonicalize_existing_dir(base_dir: &Path) -> Result<PathBuf> {
    tokio::fs::canonicalize(base_dir).await.with_context(|| {
        format!("Failed to resolve plugin base dir {base_dir}", base_dir = base_dir.display())
    })
}

/// Creates and canonicalizes a directory, ensuring it stays under the base dir.
///
/// # Errors
///
/// Returns an error if the directory cannot be created, canonicalized, or is outside the base
/// dir.
pub async fn ensure_dir_under(base_real: &Path, dir: &Path, label: &str) -> Result<PathBuf> {
    // Resolve the target's canonical location before creating anything so an out-of-base path is
    // rejected with no filesystem side effect.
    let dir_real = resolve_dir_under_base(base_real, dir, label).await?;
    tokio::fs::create_dir_all(&dir_real)
        .await
        .with_context(|| format!("Failed to create {label} dir {dir}", dir = dir_real.display()))?;
    Ok(dir_real)
}

/// Resolves where `dir` would canonically live and ensures it is under `base_real` without
/// creating it, by canonicalizing the deepest existing ancestor and re-attaching the missing tail.
async fn resolve_dir_under_base(base_real: &Path, dir: &Path, label: &str) -> Result<PathBuf> {
    let mut tail: Vec<OsString> = Vec::new();
    let mut existing = dir.to_path_buf();
    let existing_real = loop {
        if let Ok(real) = tokio::fs::canonicalize(&existing).await {
            break real;
        }
        let name = existing
            .file_name()
            .map(OsString::from)
            .ok_or_else(|| anyhow!("Failed to resolve {label} dir {dir}", dir = dir.display()))?;
        tail.push(name);
        if !existing.pop() {
            return Err(anyhow!("Failed to resolve {label} dir {dir}", dir = dir.display()));
        }
    };

    let mut dir_real = existing_real;
    dir_real.extend(tail.iter().rev());
    ensure_under_base(base_real, &dir_real, label)?;
    Ok(dir_real)
}

/// Canonicalizes an existing directory and ensures it stays under the base dir.
///
/// # Errors
///
/// Returns an error if the directory cannot be canonicalized or is outside the base dir.
pub async fn ensure_existing_dir_under(
    base_real: &Path,
    dir: &Path,
    label: &str,
) -> Result<PathBuf> {
    let dir_real = tokio::fs::canonicalize(dir)
        .await
        .with_context(|| format!("Failed to resolve {label} dir {dir}", dir = dir.display()))?;
    ensure_under_base(base_real, &dir_real, label)?;
    Ok(dir_real)
}

fn ensure_under_base(base_real: &Path, dir_real: &Path, label: &str) -> Result<()> {
    if !dir_real.starts_with(base_real) {
        return Err(anyhow!(
            "{label} dir {dir_real} is outside plugin directory {base_real}",
            dir_real = dir_real.display(),
            base_real = base_real.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test fixtures use unwrap to fail loudly on setup errors
mod tests {
    use super::{
        canonicalize_existing_dir, ensure_base_dir, ensure_dir_under, ensure_existing_dir_under,
        validate_path_component,
    };
    use tempfile::TempDir;

    #[test]
    fn validate_path_component_rejects_unsafe_values() {
        for value in ["../x", "a/b", "/tmp", "", " ", "  x", "x  "] {
            assert!(validate_path_component("plugin id", value).is_err());
        }
    }

    #[test]
    fn validate_path_component_accepts_simple_name() {
        assert!(validate_path_component("plugin id", "plugin-name").is_ok());
    }

    #[tokio::test]
    async fn ensure_base_dir_creates_missing_and_returns_canonical() {
        let temp = TempDir::new().unwrap();
        let base = temp.path().join("plugins");
        assert!(!base.exists());

        let real = ensure_base_dir(&base).await.unwrap();

        assert!(base.is_dir());
        assert!(real.is_absolute());
        assert_eq!(real, tokio::fs::canonicalize(&base).await.unwrap());
    }

    #[tokio::test]
    async fn canonicalize_existing_dir_ok_and_missing_errors() {
        let temp = TempDir::new().unwrap();
        let real = canonicalize_existing_dir(temp.path()).await.unwrap();
        assert_eq!(real, tokio::fs::canonicalize(temp.path()).await.unwrap());

        let missing = temp.path().join("does-not-exist");
        assert!(canonicalize_existing_dir(&missing).await.is_err());
    }

    #[tokio::test]
    async fn ensure_dir_under_creates_when_under_base() {
        let temp = TempDir::new().unwrap();
        let base = ensure_base_dir(temp.path()).await.unwrap();
        let target = base.join("models");

        let real = ensure_dir_under(&base, &target, "models").await.unwrap();

        assert!(target.is_dir());
        assert!(real.starts_with(&base));
    }

    #[tokio::test]
    async fn ensure_dir_under_rejects_path_outside_base() {
        let temp = TempDir::new().unwrap();
        let base = ensure_base_dir(&temp.path().join("base")).await.unwrap();
        let outside = temp.path().join("outside");

        let err = ensure_dir_under(&base, &outside, "models").await.unwrap_err();

        assert!(err.to_string().contains("outside plugin directory"));
        assert!(!outside.exists(), "out-of-base dir must not be created when validation fails");
    }

    #[tokio::test]
    async fn ensure_existing_dir_under_ok_outside_and_missing() {
        let temp = TempDir::new().unwrap();
        let base = ensure_base_dir(&temp.path().join("base")).await.unwrap();

        let under = base.join("models");
        tokio::fs::create_dir_all(&under).await.unwrap();
        let real = ensure_existing_dir_under(&base, &under, "models").await.unwrap();
        assert!(real.starts_with(&base));

        let outside = temp.path().join("outside");
        tokio::fs::create_dir_all(&outside).await.unwrap();
        let err = ensure_existing_dir_under(&base, &outside, "models").await.unwrap_err();
        assert!(err.to_string().contains("outside plugin directory"));

        let missing = base.join("missing");
        assert!(ensure_existing_dir_under(&base, &missing, "models").await.is_err());
    }
}
