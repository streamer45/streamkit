// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use streamkit_core::constraints::GlobalNodeConstraints;
use streamkit_core::{NodeRegistry, ProcessorNode};

pub mod bytes_input;
pub mod bytes_output;
pub mod file_read;
pub mod file_write;
mod glob_filter;
pub mod json_serialize;
#[cfg(feature = "object_store")]
pub mod object_store_write;
pub mod pacer;
pub mod param_bridge;
mod passthrough;
#[cfg(feature = "script")]
pub mod script;
pub mod sink;
pub mod telemetry_out;
pub mod telemetry_tap;
pub mod text_chunker;
use passthrough::PassthroughNode;
use streamkit_core::registry::StaticPins;

/// # Panics
/// Panics if default configs or JSON schemas fail to serialize.
#[allow(clippy::expect_used)] // Schema serialization should never fail
pub fn register_core_nodes(registry: &mut NodeRegistry, constraints: &GlobalNodeConstraints) {
    let _ = constraints;

    #[cfg(feature = "passthrough")]
    {
        use schemars::JsonSchema;
        use serde::Deserialize;

        #[derive(Deserialize, Debug, Default, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        pub struct PassthroughConfig {}

        register_static_node!(
            registry,
            "core::passthrough",
            |_params| Ok(Box::new(PassthroughNode)),
            PassthroughConfig,
            StaticPins {
                inputs: PassthroughNode.input_pins(),
                outputs: PassthroughNode.output_pins(),
            },
            ["core"],
            "Forwards packets unchanged. Useful for pipeline debugging, branching, \
             or as a placeholder during development.",
        );
    }

    #[cfg(feature = "file_io")]
    {
        register_dynamic_node!(
            registry,
            "core::file_reader",
            file_read::FileReadNode,
            file_read::FileReadConfig,
            ["io", "file"],
            "Reads binary data from a file and emits it as packets. \
             Supports configurable chunk sizes for streaming large files.",
        );

        register_dynamic_node!(
            registry, "core::file_writer",
            file_write::FileWriteNode, file_write::FileWriteConfig,
            ["io", "file"],
            "Writes incoming binary packets to a file. \
             Security: the server validates write paths against `security.allowed_write_paths` (default deny).",
        );
    }

    #[cfg(feature = "pacer")]
    {
        register_dynamic_node!(
            registry,
            "core::pacer",
            pacer::PacerNode,
            pacer::PacerConfig,
            ["core", "timing"],
            "Controls packet flow rate by releasing packets at specified intervals. \
             Useful for rate-limiting or simulating real-time data streams.",
        );
    }

    register_static_node!(
        registry,
        "core::json_serialize",
        |params| Ok(Box::new(json_serialize::JsonSerialize::new(params)?)),
        json_serialize::JsonSerializeConfig,
        StaticPins {
            inputs: json_serialize::JsonSerialize::input_pins(),
            outputs: json_serialize::JsonSerialize::output_pins(),
        },
        ["core", "serialization"],
        "Converts structured packets (Text, Transcription) to JSON-formatted text. \
         Useful for logging, debugging, or sending data to external services.",
    );

    register_dynamic_node!(
        registry,
        "core::text_chunker",
        text_chunker::TextChunkerNode,
        text_chunker::TextChunkerConfig,
        ["core", "text"],
        "Splits text into smaller chunks at sentence or clause boundaries. \
         Essential for streaming TTS where text should be spoken as it arrives \
         rather than waiting for complete paragraphs.",
    );

    sink::register(registry);

    #[cfg(feature = "script")]
    {
        let global_config = constraints.get::<script::GlobalScriptConfig>().cloned();

        // Parametrized factory — not suitable for register_dynamic_node! macro
        let factory = script::ScriptNode::factory(global_config);
        registry.register_dynamic_with_description(
            "core::script",
            move |params| (factory)(params),
            serde_json::to_value(schemars::schema_for!(script::ScriptConfig))
                .expect("ScriptConfig schema should serialize to JSON"),
            vec!["core".to_string(), "scripting".to_string()],
            false,
            "Execute custom JavaScript code for API integration, webhooks, text transformation, and dynamic routing. \
             Provides a sandboxed QuickJS runtime with fetch() API support. \
             See the [Script Node Guide](/guides/script-node/) for detailed usage.",
        );
    }

    // Free-function factory — not suitable for register_dynamic_node! macro
    registry.register_dynamic_with_description(
        "core::telemetry_tap",
        telemetry_tap::create_telemetry_tap,
        serde_json::to_value(schemars::schema_for!(telemetry_tap::TelemetryTapConfig))
            .expect("TelemetryTapConfig schema should serialize to JSON"),
        vec!["core".to_string(), "observability".to_string()],
        false,
        "Observes packets and emits telemetry events for debugging and timeline visualization. \
         Packets pass through unchanged while side-effect telemetry is sent to the session bus. \
         Useful for monitoring Transcription, Custom (VAD), and other packet types.",
    );

    telemetry_out::register(registry);

    param_bridge::register(registry);

    #[cfg(feature = "object_store")]
    {
        register_dynamic_node!(
            registry, "core::object_store_writer",
            object_store_write::ObjectStoreWriteNode, object_store_write::ObjectStoreWriteConfig,
            ["io", "object_store"],
            "Streams binary data to S3-compatible object storage (AWS S3, GCS, Azure, MinIO, RustFS, etc.). \
             Uses multipart upload for bounded memory usage. \
             Credentials can be provided via config or environment variables. \
             Set passthrough: true to forward packets downstream (required for oneshot pipelines).",
        );
    }
}
