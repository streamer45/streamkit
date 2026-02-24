// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Video nodes and registration.

use streamkit_core::NodeRegistry;

#[cfg(feature = "colorbars")]
pub mod colorbars;

#[cfg(feature = "compositor")]
pub mod compositor;

#[cfg(feature = "vp9")]
pub mod vp9;

/// Registers all available video nodes with the engine's registry.
#[allow(clippy::missing_const_for_fn)]
pub fn register_video_nodes(registry: &mut NodeRegistry) {
    #[cfg(feature = "colorbars")]
    colorbars::register_colorbars_nodes(registry);

    #[cfg(feature = "compositor")]
    compositor::register_compositor_nodes(registry);

    #[cfg(feature = "vp9")]
    vp9::register_vp9_nodes(registry);
}
