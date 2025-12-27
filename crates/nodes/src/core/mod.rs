// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::{NodeRegistry, ProcessorNode};

pub mod bytes_input;
pub mod bytes_output;
pub mod file_read;
pub mod file_write;
pub mod json_serialize;
pub mod pacer;
mod passthrough;
#[cfg(feature = "script")]
pub mod script;
pub mod sink;
pub mod telemetry_out;
pub mod telemetry_tap;
pub mod text_chunker;
use passthrough::PassthroughNode;
use streamkit_core::registry::StaticPins;

/// Registers all available core nodes with the engine's main registry.
///
/// Note: This does not register the special-purpose input/output nodes,
/// as they are instantiated manually by the stateless runner.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
#[allow(clippy::implicit_hasher)]
#[cfg(feature = "script")]
pub fn register_core_nodes(
    registry: &mut NodeRegistry,
    global_script_allowlist: Option<Vec<script::AllowlistRule>>,
    secrets: std::collections::HashMap<String, script::ScriptSecret>,
) {
    // --- Register PassthroughNode ---
    #[cfg(feature = "passthrough")]
    {
        use schemars::{schema_for, JsonSchema};
        use serde::Deserialize;

        #[derive(Deserialize, Debug, Default, JsonSchema)]
        #[serde(default)]
        pub struct PassthroughConfig {}

        registry.register_static_with_description(
            "core::passthrough",
            |_params| Ok(Box::new(PassthroughNode)),
            serde_json::to_value(schema_for!(PassthroughConfig))
                .expect("PassthroughConfig schema should serialize to JSON"),
            StaticPins {
                inputs: PassthroughNode.input_pins(),
                outputs: PassthroughNode.output_pins(),
            },
            vec!["core".to_string()],
            false,
            "Forwards packets unchanged. Useful for pipeline debugging, branching, \
             or as a placeholder during development.",
        );
    }

    // --- Register FileReadNode and FileWriteNode ---
    #[cfg(feature = "file_io")]
    {
        use schemars::schema_for;

        let factory = file_read::FileReadNode::factory();
        registry.register_dynamic_with_description(
            "core::file_reader",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(file_read::FileReadConfig))
                .expect("FileReadConfig schema should serialize to JSON"),
            vec!["core".to_string(), "io".to_string()],
            false,
            "Reads binary data from a file and emits it as packets. \
             Supports configurable chunk sizes for streaming large files.",
        );

        let factory = file_write::FileWriteNode::factory();
        registry.register_dynamic_with_description(
            "core::file_writer",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(file_write::FileWriteConfig))
                .expect("FileWriteConfig schema should serialize to JSON"),
            vec!["core".to_string(), "io".to_string()],
            false,
            "Writes incoming binary packets to a file. \
             Security: the server validates write paths against `security.allowed_write_paths` (default deny).",
        );
    }

    // --- Register PacerNode ---
    #[cfg(feature = "pacer")]
    {
        use schemars::schema_for;

        let factory = pacer::PacerNode::factory();
        registry.register_dynamic_with_description(
            "core::pacer",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(pacer::PacerConfig))
                .expect("PacerConfig schema should serialize to JSON"),
            vec!["core".to_string(), "timing".to_string()],
            false,
            "Controls packet flow rate by releasing packets at specified intervals. \
             Useful for rate-limiting or simulating real-time data streams.",
        );
    }

    // --- Register JsonSerialize ---
    {
        use schemars::schema_for;

        registry.register_static_with_description(
            "core::json_serialize",
            |params| Ok(Box::new(json_serialize::JsonSerialize::new(params)?)),
            serde_json::to_value(schema_for!(json_serialize::JsonSerializeConfig))
                .expect("JsonSerializeConfig schema should serialize to JSON"),
            StaticPins {
                inputs: json_serialize::JsonSerialize::input_pins(),
                outputs: json_serialize::JsonSerialize::output_pins(),
            },
            vec!["core".to_string(), "serialization".to_string()],
            false,
            "Converts structured packets (Text, Transcription) to JSON-formatted text. \
             Useful for logging, debugging, or sending data to external services.",
        );
    }

    // --- Register TextChunker ---
    {
        use schemars::schema_for;

        let factory = text_chunker::TextChunkerNode::factory();
        registry.register_dynamic_with_description(
            "core::text_chunker",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(text_chunker::TextChunkerConfig))
                .expect("TextChunkerConfig schema should serialize to JSON"),
            vec!["core".to_string(), "text".to_string()],
            false,
            "Splits text into smaller chunks at sentence or clause boundaries. \
             Essential for streaming TTS where text should be spoken as it arrives \
             rather than waiting for complete paragraphs.",
        );
    }

    // --- Register Sink Node ---
    sink::register(registry);

    // --- Register Script Node ---
    #[cfg(feature = "script")]
    {
        use schemars::schema_for;

        // Convert global allowlist and secrets to GlobalScriptConfig
        let global_config = global_script_allowlist.map(|allowlist| script::GlobalScriptConfig {
            global_fetch_allowlist: allowlist,
            secrets,
        });

        let factory = script::ScriptNode::factory(global_config);
        registry.register_dynamic_with_description(
            "core::script",
            move |params| (factory)(params),
            serde_json::to_value(schema_for!(script::ScriptConfig))
                .expect("ScriptConfig schema should serialize to JSON"),
            vec!["core".to_string(), "scripting".to_string()],
            false,
            "Execute custom JavaScript code for API integration, webhooks, text transformation, and dynamic routing. \
             Provides a sandboxed QuickJS runtime with fetch() API support. \
             See the [Script Node Guide](/guides/script-node/) for detailed usage.",
        );
    }

    // --- Register TelemetryTap Node ---
    {
        use schemars::schema_for;

        registry.register_dynamic_with_description(
            "core::telemetry_tap",
            telemetry_tap::create_telemetry_tap,
            serde_json::to_value(schema_for!(telemetry_tap::TelemetryTapConfig))
                .expect("TelemetryTapConfig schema should serialize to JSON"),
            vec!["core".to_string(), "observability".to_string()],
            false,
            "Observes packets and emits telemetry events for debugging and timeline visualization. \
             Packets pass through unchanged while side-effect telemetry is sent to the session bus. \
             Useful for monitoring Transcription, Custom (VAD), and other packet types.",
        );
    }

    // --- Register TelemetryOut Node ---
    telemetry_out::register(registry);
}

/// Registers all available core nodes with the engine's main registry (without script config).
///
/// Note: This does not register the special-purpose input/output nodes,
/// as they are instantiated manually by the stateless runner.
///
/// # Panics
///
/// Panics if config schemas cannot be serialized to JSON (should never happen).
#[allow(clippy::expect_used)] // Schema serialization should never fail for valid types
#[cfg(not(feature = "script"))]
pub fn register_core_nodes(registry: &mut NodeRegistry) {
    // --- Register PassthroughNode ---
    #[cfg(feature = "passthrough")]
    {
        use schemars::{schema_for, JsonSchema};
        use serde::Deserialize;

        #[derive(Deserialize, Debug, Default, JsonSchema)]
        #[serde(default)]
        pub struct PassthroughConfig {}

        registry.register_static_with_description(
            "core::passthrough",
            PassthroughNode::new,
            serde_json::to_value(schema_for!(PassthroughConfig))
                .expect("PassthroughConfig schema should serialize to JSON"),
            vec!["core".to_string()],
            false,
            "Passes packets through unchanged. Useful for testing or as a no-op placeholder.",
        );
    }

    // --- Register Other Core Nodes ---
    text_chunker::register(registry);
    bytes_input::register(registry);
    bytes_output::register(registry);
    json_serialize::register(registry);
    pacer::register(registry);
    file_read::register(registry);
    file_write::register(registry);
    sink::register(registry);

    // --- Register TelemetryTap Node ---
    {
        use schemars::schema_for;

        registry.register_dynamic_with_description(
            "core::telemetry_tap",
            telemetry_tap::create_telemetry_tap,
            serde_json::to_value(schema_for!(telemetry_tap::TelemetryTapConfig))
                .expect("TelemetryTapConfig schema should serialize to JSON"),
            vec!["core".to_string(), "observability".to_string()],
            false,
            "Observes packets and emits telemetry events for debugging and timeline visualization. \
             Packets pass through unchanged while side-effect telemetry is sent to the session bus. \
             Useful for monitoring Transcription, Custom (VAD), and other packet types.",
        );
    }

    // --- Register TelemetryOut Node ---
    telemetry_out::register(registry);

    tracing::info!("Finished registering core nodes (without script).");
}
