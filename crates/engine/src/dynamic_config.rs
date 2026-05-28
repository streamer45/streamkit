// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Configuration and constants for the dynamic engine.

use crate::constants::DEFAULT_BATCH_SIZE;

pub use crate::constants::DEFAULT_CONTROL_CAPACITY as CONTROL_CAPACITY;

/// Configuration for the dynamic engine actor.
#[derive(Debug, Clone)]
pub struct DynamicEngineConfig {
    pub packet_batch_size: usize,
    pub session_id: Option<String>,
    /// Overrides `DEFAULT_NODE_INPUT_CAPACITY` when `Some`.
    pub node_input_capacity: Option<usize>,
    /// Overrides `DEFAULT_PIN_DISTRIBUTOR_CAPACITY` when `Some`.
    pub pin_distributor_capacity: Option<usize>,
    /// Root directory for resolving relative asset paths in nodes.
    pub asset_root: std::path::PathBuf,
}

impl DynamicEngineConfig {
    pub fn new(asset_root: std::path::PathBuf) -> Self {
        Self { asset_root, ..Self::default() }
    }
}

/// Note: `Default` calls `std::env::current_dir()` to populate `asset_root`.
/// Prefer [`DynamicEngineConfig::new`] when the asset root is known.
impl Default for DynamicEngineConfig {
    fn default() -> Self {
        Self {
            packet_batch_size: DEFAULT_BATCH_SIZE,
            session_id: None,
            node_input_capacity: None,
            pin_distributor_capacity: None,
            asset_root: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
        }
    }
}
