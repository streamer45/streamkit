// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Default font paths for the compositor and colorbars nodes.
//!
//! Fonts are loaded from disk at runtime as system font assets rather than
//! being embedded in the binary via `include_bytes!`.  The default fonts
//! (DejaVu family) ship as system assets in `samples/fonts/system/`.

use std::path::Path;

/// Default proportional font asset path (DejaVu Sans) — used when no font is
/// specified in compositor text overlays.
pub const DEFAULT_FONT_PATH: &str = "samples/fonts/system/DejaVuSans.ttf";

/// Default monospace font asset path (DejaVu Sans Mono) — used by the
/// colorbars `draw_time` overlay.
pub const DEFAULT_MONO_FONT_PATH: &str = "samples/fonts/system/DejaVuSansMono.ttf";

/// Load the default proportional font bytes from disk.
///
/// Returns an error if the file cannot be read.
pub fn load_default_font(asset_root: &Path) -> Result<Vec<u8>, String> {
    let full_path = asset_root.join(DEFAULT_FONT_PATH);
    std::fs::read(&full_path)
        .map_err(|e| format!("Failed to read default font '{}': {e}", full_path.display()))
}

/// Load the default monospace font bytes from disk.
///
/// Returns an error if the file cannot be read.
pub fn load_default_mono_font(asset_root: &Path) -> Result<Vec<u8>, String> {
    let full_path = asset_root.join(DEFAULT_MONO_FONT_PATH);
    std::fs::read(&full_path)
        .map_err(|e| format!("Failed to read default mono font '{}': {e}", full_path.display()))
}
