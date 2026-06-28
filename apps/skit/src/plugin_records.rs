// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::{marketplace::PluginKind, plugin_paths};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivePluginRecord {
    pub plugin_id: String,
    pub version: String,
    pub node_kind: String,
    pub kind: PluginKind,
    pub entrypoint: String,
    pub installed_at_ms: u128,
    /// Accelerator variant of the activated bundle (e.g. `cpu`, `cuda`).
    /// Empty on records written before bundles were keyed by accelerator.
    #[serde(default)]
    pub accelerator: String,
}

pub fn active_dir(plugin_dir: &Path) -> PathBuf {
    plugin_dir.join("active")
}

/// Builds the active plugin record path for a given plugin id.
///
/// # Errors
///
/// Returns an error if the plugin id is not a single safe path component.
pub fn record_path(plugin_dir: &Path, plugin_id: &str) -> Result<PathBuf> {
    plugin_paths::validate_path_component("plugin id", plugin_id)?;
    Ok(active_dir(plugin_dir).join(format!("{plugin_id}.json")))
}

pub fn namespaced_kind(record: &ActivePluginRecord) -> String {
    match record.kind {
        PluginKind::Wasm => {
            format!("plugin::wasm::{node_kind}", node_kind = record.node_kind)
        },
        PluginKind::Native => {
            format!("plugin::native::{node_kind}", node_kind = record.node_kind)
        },
    }
}
