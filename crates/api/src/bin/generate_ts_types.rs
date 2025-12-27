// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Allowed: This is a CLI code generation tool, not server code.
// Using println! for progress output is appropriate here.
#![allow(clippy::disallowed_macros)]

use std::fs;
use std::path::Path;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::types::{
    AudioFormat, PacketMetadata, PacketType, SampleFormat, TranscriptionData, TranscriptionSegment,
};
use ts_rs::TS;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let declarations = vec![
        // streamkit-core types
        format!("// streamkit-core\nexport {}", SampleFormat::decl()),
        format!("export {}", AudioFormat::decl()),
        format!("export {}", PacketMetadata::decl()),
        format!("export {}", TranscriptionSegment::decl()),
        format!("export {}", TranscriptionData::decl()),
        format!("export {}", PacketType::decl()),
        format!("export {}", streamkit_core::PinCardinality::decl()),
        format!("export {}", streamkit_core::InputPin::decl()),
        format!("export {}", streamkit_core::OutputPin::decl()),
        format!("export {}", streamkit_core::NodeDefinition::decl()),
        format!("export {}", streamkit_core::StopReason::decl()),
        format!("export {}", streamkit_core::NodeState::decl()),
        format!("export {}", streamkit_core::NodeStats::decl()),
        format!("export {}", NodeControlMessage::decl()),
        // packet type registry metadata (server-driven UI)
        format!("export {}", streamkit_core::packet_meta::FieldRule::decl()),
        format!("export {}", streamkit_core::packet_meta::Compatibility::decl()),
        format!("export {}", streamkit_core::packet_meta::PacketTypeMeta::decl()),
        // streamkit-api types
        format!("\n// streamkit-api\nexport {}", streamkit_api::MessageType::decl()),
        format!("export {}", streamkit_api::RequestPayload::decl()),
        format!("export {}", streamkit_api::ResponsePayload::decl()),
        format!("export {}", streamkit_api::EventPayload::decl()),
        format!("export {}", streamkit_api::SessionInfo::decl()),
        format!("export {}", streamkit_api::EngineMode::decl()),
        format!("export {}", streamkit_api::ConnectionMode::decl()),
        format!("export {}", streamkit_api::Connection::decl()),
        format!("export {}", streamkit_api::Node::decl()),
        format!("export {}", streamkit_api::Pipeline::decl()),
        format!("export {}", streamkit_api::SamplePipeline::decl()),
        format!("export {}", streamkit_api::SavePipelineRequest::decl()),
        format!("export {}", streamkit_api::AudioAsset::decl()),
        format!("export {}", streamkit_api::BatchOperation::decl()),
        format!("export {}", streamkit_api::ValidationError::decl()),
        format!("export {}", streamkit_api::ValidationErrorType::decl()),
        format!("export {}", streamkit_api::PermissionsInfo::decl()),
    ];

    let output = declarations.join("\n\n");
    let content = format!(
        "// This file is auto-generated. Do not edit it manually.\n\n// Keep loose to allow schema usage in UI\nexport type JsonValue = unknown;\n\n{output}"
    );

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(|parent| parent.parent())
        .ok_or("Failed to find workspace root from CARGO_MANIFEST_DIR")?;
    let output_path = workspace_root.join("ui/src/types/generated/api-types.ts");

    println!("Writing TypeScript bindings to: {}", output_path.display());

    fs::write(&output_path, content)?;

    println!("✅ TypeScript bindings generated successfully.");

    Ok(())
}
