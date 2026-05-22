// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Tests for `Engine` construction and run-mode entry points in `lib.rs`.

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// setup failures (lock poisoning, registry lookups). No production behavior.
#![allow(clippy::expect_used)]

use crate::oneshot::{OneshotEngineConfig, OneshotInput};
use crate::Engine;
use bytes::Bytes;
use streamkit_api::{EngineMode, Node, Pipeline};
use streamkit_core::error::StreamKitError;
use streamkit_core::registry::NodeRegistry;

type DummyStream = futures::stream::Empty<Result<Bytes, std::io::Error>>;

fn engine_with_registry(registry: NodeRegistry) -> Engine {
    Engine {
        registry: std::sync::Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: std::sync::Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: std::sync::Arc::new(streamkit_core::VideoFramePool::video_default()),
    }
}

#[test]
fn engine_new_registers_builtin_nodes() {
    // Point at an explicit, non-existent plugin directory to keep the test
    // hermetic. `Engine::default()` would scan `./plugins` relative to the
    // process CWD, coupling this unit test to workspace filesystem state
    // (and incurring real WASM I/O). Plugin loading is exercised by the
    // server's integration tests; here we only assert that the built-in
    // node registration path runs.
    let engine = Engine::with_plugin_dir(Some(std::path::PathBuf::from(
        "/this/path/intentionally/does/not/exist/engine_construction_test",
    )));
    let registered = {
        let registry =
            engine.registry.read().expect("registry lock should not be poisoned in a fresh Engine");
        registry.definitions().iter().any(|def| def.kind == "core::file_reader")
    };
    assert!(registered, "core::file_reader should be registered after Engine::new()");
}

#[test]
fn engine_without_plugins_skips_wasm_loading() {
    let engine = Engine::without_plugins();
    let registered = {
        let registry = engine.registry.read().expect("registry lock");
        registry.definitions().iter().any(|def| def.kind == "core::file_reader")
    };
    assert!(registered, "built-in nodes must remain registered when plugins are disabled");
}

#[tokio::test]
async fn run_oneshot_with_empty_registry_fails_to_build_pipeline() {
    let engine = engine_with_registry(NodeRegistry::new());

    let mut nodes = indexmap::IndexMap::new();
    nodes.insert(
        "unknown".to_string(),
        Node { kind: "nonexistent::ghost".to_string(), params: None, state: None },
    );
    nodes.insert(
        "sink".to_string(),
        Node { kind: "streamkit::http_output".to_string(), params: None, state: None },
    );

    let definition = Pipeline {
        name: None,
        description: None,
        mode: EngineMode::OneShot,
        client: None,
        nodes,
        connections: vec![streamkit_api::Connection {
            from_node: "unknown".to_string(),
            from_pin: "out".to_string(),
            to_node: "sink".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        }],
        view_data: None,
        runtime_schemas: None,
    };

    let inputs: Vec<OneshotInput<DummyStream>> = Vec::new();
    let result = engine
        .run_oneshot_pipeline(definition, inputs, Some(OneshotEngineConfig::default()), None)
        .await;
    // Pin to the specific variant + message substring that the registry
    // surfaces when a referenced node kind is missing. A regression that
    // re-routes this to `Configuration(_)` or to a different message would
    // otherwise pass the previous OR-matcher silently.
    let err = result.err().expect("expected unknown-node-kind failure");
    assert!(
        matches!(&err, StreamKitError::Runtime(msg) if msg.contains("nonexistent::ghost")
            && msg.contains("not found in registry")),
        "expected Runtime error mentioning the missing node kind; got {err:?}"
    );
}

#[cfg(feature = "dynamic")]
#[tokio::test]
async fn start_dynamic_actor_returns_responsive_handle() {
    use crate::DynamicEngineConfig;

    let engine = engine_with_registry(NodeRegistry::new());
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());

    let states = handle.get_node_states().await.expect("get_node_states");
    assert!(states.is_empty(), "no nodes have been added yet");

    let shutdown =
        tokio::time::timeout(std::time::Duration::from_secs(5), handle.shutdown_and_wait())
            .await
            .expect("shutdown should not hang");
    shutdown.expect("clean shutdown of an empty dynamic engine");
}
