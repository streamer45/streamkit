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
    AudioCodec, AudioFormat, EncodedAudioFormat, EncodedVideoFormat, PacketMetadata, PacketType,
    PixelFormat, RawVideoFormat, SampleFormat, TranscriptionData, TranscriptionSegment,
    VideoBitstreamFormat, VideoCodec,
};
use ts_rs::{Config, TS};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cfg = Config::default();
    let declarations = vec![
        // streamkit-core types
        format!("// streamkit-core\nexport {}", SampleFormat::decl(&cfg)),
        format!("export {}", AudioFormat::decl(&cfg)),
        format!("export {}", PixelFormat::decl(&cfg)),
        format!("export {}", RawVideoFormat::decl(&cfg)),
        format!("export {}", AudioCodec::decl(&cfg)),
        format!("export {}", VideoCodec::decl(&cfg)),
        format!("export {}", VideoBitstreamFormat::decl(&cfg)),
        format!("export {}", EncodedAudioFormat::decl(&cfg)),
        format!("export {}", EncodedVideoFormat::decl(&cfg)),
        format!("export {}", PacketMetadata::decl(&cfg)),
        format!("export {}", TranscriptionSegment::decl(&cfg)),
        format!("export {}", TranscriptionData::decl(&cfg)),
        format!("export {}", PacketType::decl(&cfg)),
        format!("export {}", streamkit_core::PinCardinality::decl(&cfg)),
        format!("export {}", streamkit_core::InputPin::decl(&cfg)),
        format!("export {}", streamkit_core::OutputPin::decl(&cfg)),
        format!("export {}", streamkit_core::NodeDefinition::decl(&cfg)),
        format!("export {}", streamkit_core::StopReason::decl(&cfg)),
        format!("export {}", streamkit_core::NodeState::decl(&cfg)),
        format!("export {}", streamkit_core::NodeStats::decl(&cfg)),
        format!("export {}", NodeControlMessage::decl(&cfg)),
        // packet type registry metadata (server-driven UI)
        format!("export {}", streamkit_core::packet_meta::FieldRule::decl(&cfg)),
        format!("export {}", streamkit_core::packet_meta::Compatibility::decl(&cfg)),
        format!("export {}", streamkit_core::packet_meta::PacketTypeMeta::decl(&cfg)),
        // streamkit-api types
        format!("\n// streamkit-api\nexport {}", streamkit_api::MessageType::decl(&cfg)),
        format!("export {}", streamkit_api::RequestPayload::decl(&cfg)),
        format!("export {}", streamkit_api::ResponsePayload::decl(&cfg)),
        format!("export {}", streamkit_api::EventPayload::decl(&cfg)),
        format!("export {}", streamkit_api::SessionInfo::decl(&cfg)),
        format!("export {}", streamkit_api::EngineMode::decl(&cfg)),
        format!("export {}", streamkit_api::ConnectionMode::decl(&cfg)),
        format!("export {}", streamkit_api::Connection::decl(&cfg)),
        format!("export {}", streamkit_api::Node::decl(&cfg)),
        format!("export {}", streamkit_api::Pipeline::decl(&cfg)),
        format!("export {}", streamkit_api::SamplePipeline::decl(&cfg)),
        format!("export {}", streamkit_api::SavePipelineRequest::decl(&cfg)),
        format!("export {}", streamkit_api::AudioAsset::decl(&cfg)),
        format!("export {}", streamkit_api::BatchOperation::decl(&cfg)),
        format!("export {}", streamkit_api::ValidationError::decl(&cfg)),
        format!("export {}", streamkit_api::ValidationErrorType::decl(&cfg)),
        format!("export {}", streamkit_api::PermissionsInfo::decl(&cfg)),
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
