// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allowed: This is a CLI code generation tool, not server code.
// Using println! for progress output is appropriate here.
#![allow(clippy::disallowed_macros)]

use std::fs;
use std::path::Path;
use streamkit_nodes::video::compositor::config::{
    CompositorConfig, CompositorLayout, ImageOverlayConfig, LayerConfig, OverlayTransform, Rect,
    ResolvedLayer, ResolvedOverlay, TextOverlayConfig,
};
use ts_rs::TS;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = vec![
        format!("export {}", Rect::decl()),
        format!("export {}", OverlayTransform::decl()),
        format!("export {}", ImageOverlayConfig::decl()),
        format!("export {}", TextOverlayConfig::decl()),
        format!("export {}", LayerConfig::decl()),
        format!("export {}", CompositorConfig::decl()),
        format!("export {}", ResolvedLayer::decl()),
        format!("export {}", ResolvedOverlay::decl()),
        format!("export {}", CompositorLayout::decl()),
    ];

    let output = declarations.join("\n\n");
    let content = format!("// This file is auto-generated. Do not edit it manually.\n\n{output}\n");

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|parent| parent.parent())
        .ok_or("Failed to find workspace root from CARGO_MANIFEST_DIR")?;
    let output_path = workspace_root.join("ui/src/types/generated/compositor-types.ts");

    println!("Writing compositor TypeScript bindings to: {}", output_path.display());

    fs::write(&output_path, content)?;

    println!("✅ Compositor TypeScript bindings generated successfully.");

    Ok(())
}
