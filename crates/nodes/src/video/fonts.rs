// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Compile-time embedded font data for the bundled font set.
//!
//! All fonts in [`BUNDLED_FONTS`] are included in the binary via
//! `include_bytes!` so they work without any system font packages
//! installed.  The DejaVu family is distributed under the permissive
//! Bitstream Vera / DejaVu license (see `assets/fonts/LICENSE-DejaVu.txt`).

/// A font embedded in the binary at compile time.
pub struct BundledFont {
    /// User-facing name used in `font_name` config fields.
    pub name: &'static str,
    /// Raw TTF bytes baked into the binary.
    pub data: &'static [u8],
}

/// Bundled font set — always available, no filesystem dependency.
///
/// Order matters: the first entry is the default proportional font and
/// the third entry is the default monospace font (see [`DEFAULT_FONT`]
/// and [`DEFAULT_MONO_FONT`]).
pub static BUNDLED_FONTS: &[BundledFont] = &[
    BundledFont {
        name: "dejavu-sans",
        data: include_bytes!("../../../../assets/fonts/DejaVuSans.ttf"),
    },
    BundledFont {
        name: "dejavu-sans-bold",
        data: include_bytes!("../../../../assets/fonts/DejaVuSans-Bold.ttf"),
    },
    BundledFont {
        name: "dejavu-sans-mono",
        data: include_bytes!("../../../../assets/fonts/DejaVuSansMono.ttf"),
    },
    BundledFont {
        name: "dejavu-sans-mono-bold",
        data: include_bytes!("../../../../assets/fonts/DejaVuSansMono-Bold.ttf"),
    },
    BundledFont {
        name: "dejavu-serif",
        data: include_bytes!("../../../../assets/fonts/DejaVuSerif.ttf"),
    },
    BundledFont {
        name: "dejavu-serif-bold",
        data: include_bytes!("../../../../assets/fonts/DejaVuSerif-Bold.ttf"),
    },
];

/// Default proportional font bytes (DejaVu Sans) — used when no font is
/// specified in compositor text overlays.
pub static DEFAULT_FONT_DATA: &[u8] =
    include_bytes!("../../../../assets/fonts/DejaVuSans.ttf");

/// Default monospace font bytes (DejaVu Sans Mono) — used by the colorbars
/// `draw_time` overlay.
pub static DEFAULT_MONO_FONT_DATA: &[u8] =
    include_bytes!("../../../../assets/fonts/DejaVuSansMono.ttf");

/// Look up a bundled font by its user-facing name.
///
/// Returns `None` if the name is not in the bundled set.
pub fn bundled_font_by_name(name: &str) -> Option<&'static [u8]> {
    BUNDLED_FONTS.iter().find(|f| f.name == name).map(|f| f.data)
}

/// Comma-separated list of bundled font names (for error messages).
pub fn bundled_font_names() -> String {
    BUNDLED_FONTS.iter().map(|f| f.name).collect::<Vec<_>>().join(", ")
}
