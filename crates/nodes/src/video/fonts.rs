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

// ── Raw font data constants (single `include_bytes!` per file) ──────────────

static DEJAVU_SANS: &[u8] = include_bytes!("../../../../assets/fonts/DejaVuSans.ttf");
static DEJAVU_SANS_BOLD: &[u8] = include_bytes!("../../../../assets/fonts/DejaVuSans-Bold.ttf");
static DEJAVU_SANS_MONO: &[u8] = include_bytes!("../../../../assets/fonts/DejaVuSansMono.ttf");
static DEJAVU_SANS_MONO_BOLD: &[u8] =
    include_bytes!("../../../../assets/fonts/DejaVuSansMono-Bold.ttf");
static DEJAVU_SERIF: &[u8] = include_bytes!("../../../../assets/fonts/DejaVuSerif.ttf");
static DEJAVU_SERIF_BOLD: &[u8] = include_bytes!("../../../../assets/fonts/DejaVuSerif-Bold.ttf");

/// Bundled font set — always available, no filesystem dependency.
///
/// Order matters: the first entry is the default proportional font and
/// the third entry is the default monospace font (see [`DEFAULT_FONT_DATA`]
/// and [`DEFAULT_MONO_FONT_DATA`]).
pub static BUNDLED_FONTS: &[BundledFont] = &[
    BundledFont { name: "dejavu-sans", data: DEJAVU_SANS },
    BundledFont { name: "dejavu-sans-bold", data: DEJAVU_SANS_BOLD },
    BundledFont { name: "dejavu-sans-mono", data: DEJAVU_SANS_MONO },
    BundledFont { name: "dejavu-sans-mono-bold", data: DEJAVU_SANS_MONO_BOLD },
    BundledFont { name: "dejavu-serif", data: DEJAVU_SERIF },
    BundledFont { name: "dejavu-serif-bold", data: DEJAVU_SERIF_BOLD },
];

/// Default proportional font bytes (DejaVu Sans) — used when no font is
/// specified in compositor text overlays.
pub static DEFAULT_FONT_DATA: &[u8] = DEJAVU_SANS;

/// Default monospace font bytes (DejaVu Sans Mono) — used by the colorbars
/// `draw_time` overlay.
pub static DEFAULT_MONO_FONT_DATA: &[u8] = DEJAVU_SANS_MONO;

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
