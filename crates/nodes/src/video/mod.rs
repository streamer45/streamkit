// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Video nodes and registration.

use streamkit_core::types::PixelFormat;
use streamkit_core::{NodeRegistry, StreamKitError};

/// Parse a pixel format string into a [`PixelFormat`].
///
/// Accepts `"i420"`, `"rgba8"`, or `"rgba"` (case-insensitive).
///
/// # Errors
///
/// Returns [`StreamKitError::Configuration`] if `s` is not a recognised format name.
pub fn parse_pixel_format(s: &str) -> Result<PixelFormat, StreamKitError> {
    match s.to_lowercase().as_str() {
        "i420" => Ok(PixelFormat::I420),
        "rgba8" | "rgba" => Ok(PixelFormat::Rgba8),
        other => Err(StreamKitError::Configuration(format!(
            "Unsupported pixel format '{other}'. Use 'i420' or 'rgba8'."
        ))),
    }
}

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
