// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Utility functions for node configuration and packet processing.
//!
//! This module provides helper functions that simplify common tasks:
//! - [`config_helpers`]: Parse node configuration from YAML
//! - [`packet_helpers`]: Batch packet processing utilities

use crate::error::StreamKitError;
use crate::types::Packet;

pub mod config_helpers {
    use super::StreamKitError;
    use serde::Deserialize;

    /// Parses configuration from an optional JSON value, using defaults if not provided.
    /// This is the preferred approach for nodes with sensible defaults.
    ///
    /// # Errors
    ///
    /// This function always returns `Ok` in practice, as it uses `Default` when parsing fails.
    /// The `Result` return type is maintained for API consistency with other config helpers.
    pub fn parse_config_optional<T>(params: Option<&serde_json::Value>) -> Result<T, StreamKitError>
    where
        T: for<'de> Deserialize<'de> + Default,
    {
        Ok(serde_json::from_value(params.unwrap_or(&serde_json::Value::Null).clone())
            .unwrap_or_default())
    }

    /// Parses configuration from an optional JSON value, returning an error if not provided.
    /// Use this for nodes that require explicit configuration.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Configuration` if `params` is `None` or if deserialization fails.
    pub fn parse_config_required<T>(params: Option<&serde_json::Value>) -> Result<T, StreamKitError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let value = params
            .ok_or_else(|| StreamKitError::Configuration("Configuration required".to_string()))?
            .clone();
        serde_json::from_value(value)
            .map_err(|e| StreamKitError::Configuration(format!("Failed to parse config: {e}")))
    }

    /// Parses configuration with detailed error messages.
    /// Use this when you want to provide context about what failed to parse.
    ///
    /// # Errors
    ///
    /// Returns `StreamKitError::Configuration` if `params` is `None` or if deserialization fails.
    pub fn parse_config_with_context<T>(
        params: Option<&serde_json::Value>,
        context: &str,
    ) -> Result<T, StreamKitError>
    where
        T: for<'de> Deserialize<'de>,
    {
        params.map_or_else(
            || Err(StreamKitError::Configuration(format!("{context} configuration required"))),
            |p| {
                serde_json::from_value(p.clone()).map_err(|e| {
                    StreamKitError::Configuration(format!("Failed to parse {context}: {e}"))
                })
            },
        )
    }
}

pub mod path_helpers {
    use std::path::{Path, PathBuf};

    /// Returns `true` if `path` contains traversal or absolute components
    /// (`..`, `/`, or a Windows drive prefix) that would escape `asset_root`
    /// when used with [`Path::join`].
    pub fn has_path_traversal(path: &Path) -> bool {
        path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    }

    /// Resolves a node asset path against `asset_root` without touching the
    /// filesystem.
    ///
    /// Asset paths are always relative to `asset_root`: absolute paths and `..`
    /// components are rejected. This is the single source of truth shared by the
    /// `core::file_reader`/`file_writer`/`script` nodes and the server's
    /// `file_security` validation, so the path that is validated is exactly the
    /// path that is read.
    ///
    /// # Errors
    /// Returns an error string if `path` is absolute or contains `..`.
    pub fn resolve_asset_path(path: &str, asset_root: &Path) -> Result<PathBuf, String> {
        let path_obj = Path::new(path);
        if has_path_traversal(path_obj) {
            return Err(format!(
                "path must be relative to asset_root and must not contain '..': '{path}'"
            ));
        }
        Ok(asset_root.join(path_obj))
    }

    /// An `asset_root` that has been canonicalized exactly once at the trust
    /// boundary.
    ///
    /// The path resolvers compare a canonicalized target against `asset_root`
    /// with [`Path::starts_with`], which only holds when *both* sides are
    /// canonical. Encoding "already canonical" in the type means the resolvers
    /// accept a value that is canonical by construction instead of defensively
    /// re-canonicalizing on every call (a syscall per file node, per validator).
    /// The server canonicalizes at startup; node-crate callers and embedders
    /// build one from a raw path (`current_dir()`, a `TempDir`) — which may
    /// contain symlinked components (e.g. macOS `/tmp` → `/private/tmp`) — via
    /// [`CanonicalAssetRoot::new`].
    #[derive(Clone, Debug)]
    pub struct CanonicalAssetRoot(PathBuf);

    impl CanonicalAssetRoot {
        /// Canonicalizes `asset_root`, resolving symlinks and `.`/`..`.
        ///
        /// # Errors
        /// Returns an error string if `asset_root` cannot be canonicalized
        /// (missing, inaccessible, or a permission error on a component).
        pub fn new(asset_root: &Path) -> Result<Self, String> {
            asset_root
                .canonicalize()
                .map(Self)
                .map_err(|e| format!("cannot resolve asset_root '{}': {e}", asset_root.display()))
        }

        #[must_use]
        pub fn as_path(&self) -> &Path {
            &self.0
        }
    }

    /// Resolves an existing asset path and canonicalizes it, ensuring the result
    /// is a regular file that stays within `asset_root` after symlink resolution.
    ///
    /// Canonicalizing at read time and re-checking containment closes the
    /// validate→read symlink-swap window for relative paths.
    ///
    /// # Errors
    /// Returns an error string if `path` is absolute / contains `..`, cannot be
    /// canonicalized, is not a regular file, or resolves (via symlinks) outside
    /// `asset_root`.
    pub fn resolve_existing_asset_path(
        path: &str,
        asset_root: &CanonicalAssetRoot,
    ) -> Result<PathBuf, String> {
        let root = asset_root.as_path();
        let joined = resolve_asset_path(path, root)?;
        let canonical = joined.canonicalize().map_err(|e| {
            format!("cannot resolve path '{path}' (file may not exist or is not accessible): {e}")
        })?;
        if !canonical.starts_with(root) {
            return Err(format!(
                "path '{path}' resolves to '{}' which is outside asset_root '{}'",
                canonical.display(),
                root.display()
            ));
        }
        if !canonical.is_file() {
            return Err(format!(
                "path '{path}' resolves to '{}' which is not a regular file",
                canonical.display()
            ));
        }
        Ok(canonical)
    }

    /// Resolves an asset path whose target may not exist yet (file writes),
    /// canonicalizing the parent directory and ensuring it stays within
    /// `asset_root` after symlink resolution.
    ///
    /// The target's own name is not canonicalized (it may not exist yet), but if
    /// it *already* exists as a symlink the link is resolved and required to stay
    /// within `asset_root`: otherwise a pre-existing in-root symlink
    /// (`out/report.txt` → `/etc/cron.d/payload`) would let a `File::create`
    /// follow it and truncate a file outside the root.
    ///
    /// # Errors
    /// Returns an error string if `path` is absolute / contains `..`, lacks a
    /// file name, whose parent cannot be canonicalized or resolves outside
    /// `asset_root`, or that is a symlink resolving outside `asset_root`.
    pub fn resolve_new_asset_path(
        path: &str,
        asset_root: &CanonicalAssetRoot,
    ) -> Result<PathBuf, String> {
        let root = asset_root.as_path();
        // Reject absolute / `..` first, but decompose the *relative* path so the
        // file name and parent come from the same source (deriving the file name
        // from the joined path would treat a bare `.` as a file named after the
        // root and silently escape it).
        resolve_asset_path(path, root)?;
        let rel = Path::new(path);
        let file_name = rel
            .file_name()
            .ok_or_else(|| format!("write path must include a file name: '{path}'"))?
            .to_owned();
        let parent = rel.parent().unwrap_or_else(|| Path::new(""));
        let canonical_parent = root
            .join(parent)
            .canonicalize()
            .map_err(|e| format!("cannot resolve parent directory for write path '{path}': {e}"))?;
        if !canonical_parent.starts_with(root) {
            return Err(format!(
                "write path '{path}' resolves outside asset_root '{}'",
                root.display()
            ));
        }
        let target = canonical_parent.join(file_name);
        if let Ok(meta) = std::fs::symlink_metadata(&target) {
            if meta.file_type().is_symlink() {
                let resolved = target.canonicalize().map_err(|e| {
                    format!("cannot resolve symlinked write target for path '{path}': {e}")
                })?;
                if !resolved.starts_with(root) {
                    return Err(format!(
                        "write path '{path}' is a symlink resolving to '{}' outside asset_root '{}'",
                        resolved.display(),
                        root.display()
                    ));
                }
            }
        }
        Ok(target)
    }
}

pub mod packet_helpers {
    use super::Packet;
    use smallvec::SmallVec;
    use tokio::sync::mpsc;

    /// 32 packets ≈ 1.3 KB on stack; avoids heap in the common case.
    pub const DEFAULT_BATCH_CAPACITY: usize = 32;

    pub type PacketBatch = SmallVec<[Packet; DEFAULT_BATCH_CAPACITY]>;

    /// Non-blocking drain: takes `first_packet` plus up to `batch_size - 1` more.
    pub fn batch_packets_greedy(
        first_packet: Packet,
        rx: &mut mpsc::Receiver<Packet>,
        batch_size: usize,
    ) -> PacketBatch {
        let mut batch = PacketBatch::new();
        batch.push(first_packet);

        for _ in 0..batch_size.saturating_sub(1) {
            match rx.try_recv() {
                Ok(packet) => batch.push(packet),
                Err(_) => break,
            }
        }
        batch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, Default, PartialEq)]
    struct TestConfig {
        #[serde(default)]
        gain: f32,
        #[serde(default)]
        channels: u16,
    }

    #[test]
    fn parse_config_optional_with_valid_json() {
        let params = serde_json::json!({"gain": 0.5, "channels": 2});
        let cfg: TestConfig = config_helpers::parse_config_optional(Some(&params)).unwrap();
        assert_eq!(cfg.gain, 0.5);
        assert_eq!(cfg.channels, 2);
    }

    #[test]
    fn parse_config_optional_with_none_returns_default() {
        let cfg: TestConfig = config_helpers::parse_config_optional(None).unwrap();
        assert_eq!(cfg, TestConfig::default());
    }

    #[test]
    fn parse_config_optional_with_partial_json_fills_defaults() {
        let params = serde_json::json!({"gain": 1.5});
        let cfg: TestConfig = config_helpers::parse_config_optional(Some(&params)).unwrap();
        assert_eq!(cfg.gain, 1.5);
        assert_eq!(cfg.channels, 0);
    }

    #[test]
    fn parse_config_required_with_valid_json() {
        let params = serde_json::json!({"gain": 2.0, "channels": 1});
        let cfg: TestConfig = config_helpers::parse_config_required(Some(&params)).unwrap();
        assert_eq!(cfg.gain, 2.0);
        assert_eq!(cfg.channels, 1);
    }

    #[test]
    fn parse_config_required_with_none_returns_error() {
        let result = config_helpers::parse_config_required::<TestConfig>(None);
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("Configuration"), "expected Configuration error, got: {err_str}");
    }

    #[test]
    fn parse_config_required_with_invalid_type_returns_error() {
        let params = serde_json::json!({"gain": "not_a_number"});
        let result = config_helpers::parse_config_required::<TestConfig>(Some(&params));
        assert!(result.is_err());
    }

    #[test]
    fn parse_config_with_context_missing_params() {
        let result = config_helpers::parse_config_with_context::<TestConfig>(None, "AudioGain");
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("AudioGain"));
    }

    #[test]
    fn parse_config_with_context_invalid_json() {
        let params = serde_json::json!("just a string");
        let result =
            config_helpers::parse_config_with_context::<TestConfig>(Some(&params), "AudioGain");
        assert!(result.is_err());
        let err_str = result.unwrap_err().to_string();
        assert!(err_str.contains("AudioGain"));
    }

    #[test]
    fn parse_config_optional_with_invalid_type_falls_back_to_default() {
        let params = serde_json::json!({"gain": "not_a_number"});
        let cfg: TestConfig = config_helpers::parse_config_optional(Some(&params)).unwrap();
        assert_eq!(cfg, TestConfig::default());
    }

    #[test]
    fn parse_config_with_context_valid_json() {
        let params = serde_json::json!({"gain": 3.0, "channels": 4});
        let cfg: TestConfig =
            config_helpers::parse_config_with_context(Some(&params), "AudioGain").unwrap();
        assert_eq!(cfg.gain, 3.0);
        assert_eq!(cfg.channels, 4);
    }

    #[test]
    fn batch_packets_greedy_drains_one_extra_packet() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        let first = Packet::Text(std::sync::Arc::from("hello"));
        tx.try_send(Packet::Text(std::sync::Arc::from("world"))).unwrap();
        let batch = packet_helpers::batch_packets_greedy(first, &mut rx, 4);
        assert_eq!(batch.len(), 2);
    }

    #[test]
    fn batch_packets_greedy_empty_channel() {
        let (_tx, mut rx) = tokio::sync::mpsc::channel::<Packet>(16);
        let first = Packet::Text(std::sync::Arc::from("only"));
        let batch = packet_helpers::batch_packets_greedy(first, &mut rx, 8);
        assert_eq!(batch.len(), 1);
    }

    #[test]
    fn batch_packets_greedy_respects_batch_size() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(16);
        for i in 0..10 {
            tx.try_send(Packet::Text(std::sync::Arc::from(format!("{i}")))).unwrap();
        }
        let first = Packet::Text(std::sync::Arc::from("first"));
        let batch = packet_helpers::batch_packets_greedy(first, &mut rx, 3);
        assert_eq!(batch.len(), 3);
    }

    #[test]
    fn default_batch_capacity_is_reasonable() {
        const { assert!(packet_helpers::DEFAULT_BATCH_CAPACITY >= 8) };
        const { assert!(packet_helpers::DEFAULT_BATCH_CAPACITY <= 128) };
    }

    #[test]
    fn has_path_traversal_rejects_parent_dir() {
        assert!(path_helpers::has_path_traversal(std::path::Path::new("../escape")));
    }

    #[test]
    fn has_path_traversal_rejects_absolute() {
        assert!(path_helpers::has_path_traversal(std::path::Path::new("/etc/passwd")));
    }

    #[test]
    fn has_path_traversal_accepts_relative() {
        assert!(!path_helpers::has_path_traversal(std::path::Path::new(
            "samples/audio/user/clip.opus"
        )));
    }

    #[test]
    fn has_path_traversal_accepts_dotdot_substring() {
        assert!(!path_helpers::has_path_traversal(std::path::Path::new("my..assets/file.txt")));
    }
}
