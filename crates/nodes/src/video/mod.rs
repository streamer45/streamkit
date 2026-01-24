// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Video nodes and registration.

use streamkit_core::NodeRegistry;

#[cfg(feature = "vp9")]
pub mod vp9;

/// Registers all available video nodes with the engine's registry.
#[cfg(feature = "vp9")]
pub fn register_video_nodes(registry: &mut NodeRegistry) {
    vp9::register_vp9_nodes(registry);
}

/// Registers all available video nodes with the engine's registry.
#[cfg(not(feature = "vp9"))]
pub const fn register_video_nodes(_registry: &mut NodeRegistry) {}
