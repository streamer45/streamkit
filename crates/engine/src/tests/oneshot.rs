// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// pipeline-build and runtime errors that the rest of the assertion would
// otherwise have to recover from manually.
#![allow(clippy::expect_used)]

use super::super::*;
use crate::constants::{
    DEFAULT_BATCH_SIZE, DEFAULT_ONESHOT_IO_CAPACITY, DEFAULT_ONESHOT_MEDIA_CAPACITY,
};
use crate::oneshot::{validate_input_mode, OneshotEngineConfig, OneshotInput, OneshotInputMode};
use bytes::Bytes;
use streamkit_core::control::NodeControlMessage;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

struct TextEmitterNode;

#[streamkit_core::async_trait]
impl ProcessorNode for TextEmitterNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Text,
            cardinality: PinCardinality::One,
        }]
    }
    async fn run(self: Box<Self>, mut ctx: NodeContext) -> Result<(), StreamKitError> {
        while let Some(msg) = ctx.control_rx.recv().await {
            if matches!(msg, NodeControlMessage::Start) {
                break;
            }
        }
        let _ = ctx.output_sender.send("out", Packet::Text("payload".into())).await;
        Ok(())
    }
}

struct CollectorSinkNode {
    tx: tokio::sync::mpsc::Sender<String>,
}

#[streamkit_core::async_trait]
impl ProcessorNode for CollectorSinkNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Text],
            cardinality: PinCardinality::One,
        }]
    }
    fn output_pins(&self) -> Vec<OutputPin> {
        Vec::new()
    }
    async fn run(self: Box<Self>, mut ctx: NodeContext) -> Result<(), StreamKitError> {
        let mut rx = ctx.take_input("in")?;
        while let Some(pkt) = rx.recv().await {
            if let Packet::Text(s) = pkt {
                let _ = self.tx.send(s.to_string()).await;
            }
        }
        Ok(())
    }
}

struct FailingNode;

#[streamkit_core::async_trait]
impl ProcessorNode for FailingNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Text,
            cardinality: PinCardinality::One,
        }]
    }
    async fn run(self: Box<Self>, _ctx: NodeContext) -> Result<(), StreamKitError> {
        Err(StreamKitError::Runtime("intentional failure".to_string()))
    }
}

#[test]
fn oneshot_engine_config_defaults() {
    let config = OneshotEngineConfig::default();
    assert_eq!(config.packet_batch_size, DEFAULT_BATCH_SIZE);
    assert_eq!(config.media_channel_capacity, DEFAULT_ONESHOT_MEDIA_CAPACITY);
    assert_eq!(config.io_channel_capacity, DEFAULT_ONESHOT_IO_CAPACITY);
}

#[tokio::test]
async fn linear_pipeline_runs_to_completion() {
    let (collector_tx, mut collector_rx) = tokio::sync::mpsc::channel(8);

    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("src".to_string(), Box::new(TextEmitterNode));
    nodes.insert("sink".to_string(), Box::new(CollectorSinkNode { tx: collector_tx }));

    let connections = vec![Connection {
        from_node: "src".to_string(),
        from_pin: "out".to_string(),
        to_node: "sink".to_string(),
        to_pin: "in".to_string(),
        mode: streamkit_api::ConnectionMode::Reliable,
    }];

    let node_kinds: HashMap<String, String> = [
        ("src".to_string(), "test::emitter".to_string()),
        ("sink".to_string(), "test::collector".to_string()),
    ]
    .into_iter()
    .collect();

    let Ok(live) = graph_builder::wire_and_spawn_graph(
        nodes,
        &connections,
        &node_kinds,
        1,
        DEFAULT_ONESHOT_MEDIA_CAPACITY,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    else {
        panic!("linear pipeline should wire successfully");
    };

    let Some(src) = live.get("src") else {
        panic!("source node must exist");
    };
    if let Err(e) = src.control_tx.send(NodeControlMessage::Start).await {
        panic!("sending Start should succeed: {e}");
    }

    let Ok(Some(received)) =
        tokio::time::timeout(std::time::Duration::from_secs(3), collector_rx.recv()).await
    else {
        panic!("should receive within timeout");
    };

    assert_eq!(received, "payload");

    let handles: Vec<_> = live.into_values().map(|n| n.task_handle).collect();
    let _ =
        tokio::time::timeout(std::time::Duration::from_secs(5), futures::future::join_all(handles))
            .await;
}

#[tokio::test]
async fn cancellation_token_stops_pipeline() {
    struct WaitForeverNode;

    #[streamkit_core::async_trait]
    impl ProcessorNode for WaitForeverNode {
        fn input_pins(&self) -> Vec<InputPin> {
            Vec::new()
        }
        fn output_pins(&self) -> Vec<OutputPin> {
            Vec::new()
        }
        async fn run(self: Box<Self>, ctx: NodeContext) -> Result<(), StreamKitError> {
            if let Some(token) = &ctx.cancellation_token {
                token.cancelled().await;
            }
            Ok(())
        }
    }

    let token = tokio_util::sync::CancellationToken::new();

    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("wait".to_string(), Box::new(WaitForeverNode));

    let connections: Vec<Connection> = vec![];
    let node_kinds: HashMap<String, String> =
        std::iter::once(("wait".to_string(), "test::wait".to_string())).collect();

    let Ok(live) = graph_builder::wire_and_spawn_graph(
        nodes,
        &connections,
        &node_kinds,
        1,
        DEFAULT_ONESHOT_MEDIA_CAPACITY,
        None,
        None,
        Some(token.clone()),
        None,
        None,
    )
    .await
    else {
        panic!("pipeline should wire successfully");
    };

    token.cancel();

    let handles: Vec<_> = live.into_values().map(|n| n.task_handle).collect();
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(3), futures::future::join_all(handles))
            .await;
    assert!(result.is_ok(), "cancelled nodes should finish promptly");
}

#[tokio::test]
async fn failing_node_propagates_error() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("fail".to_string(), Box::new(FailingNode));

    let connections: Vec<Connection> = vec![];
    let node_kinds: HashMap<String, String> =
        std::iter::once(("fail".to_string(), "test::failing".to_string())).collect();

    let Ok(live) = graph_builder::wire_and_spawn_graph(
        nodes,
        &connections,
        &node_kinds,
        1,
        DEFAULT_ONESHOT_MEDIA_CAPACITY,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    else {
        panic!("wiring should succeed even for a node that will fail at runtime");
    };

    let Some((_, fail_node)) = live.into_iter().next() else {
        panic!("live map should contain the failing node");
    };

    let Ok(Ok(inner)) =
        tokio::time::timeout(std::time::Duration::from_secs(3), fail_node.task_handle).await
    else {
        panic!("node should finish within timeout");
    };

    let Err(err) = inner else {
        panic!("failing node should return an error");
    };
    match err {
        StreamKitError::Runtime(msg) => {
            assert!(msg.contains("intentional failure"));
        },
        other => panic!("expected Runtime error, got: {other:?}"),
    }
}

type DummyStream = futures::stream::Empty<Result<Bytes, std::io::Error>>;

fn dummy_input(node_id: &str) -> OneshotInput<DummyStream> {
    OneshotInput {
        node_id: node_id.to_string(),
        output_pin: "out".to_string(),
        stream: futures::stream::empty(),
        content_type: None,
        field_name: "file".to_string(),
        required: true,
        cancellation_token: None,
    }
}

#[test]
fn validate_input_mode_http_with_no_inputs_errors() {
    let http_nodes = vec!["http_in".to_string()];
    let no_inputs: Vec<OneshotInput<DummyStream>> = Vec::new();
    let output = "sink".to_string();

    let result = validate_input_mode(true, &[], &http_nodes, &no_inputs, Some(&output));
    let Err(StreamKitError::Configuration(msg)) = result else {
        panic!("expected Configuration error for http_input without streams; got {result:?}");
    };
    assert!(
        msg.contains("Input streams are required"),
        "error message should mention required input streams: {msg}"
    );
}

#[test]
fn validate_input_mode_http_with_inputs_ok() {
    let http_nodes = vec!["http_in".to_string()];
    let inputs = vec![dummy_input("http_in")];

    let mode = validate_input_mode(true, &[], &http_nodes, &inputs, None)
        .expect("http_input with provided streams should be Ok");
    assert_eq!(mode, OneshotInputMode::HttpStreaming);
}

#[test]
fn validate_input_mode_file_based_ok() {
    let source_ids = vec!["file_reader".to_string()];
    let no_inputs: Vec<OneshotInput<DummyStream>> = Vec::new();

    let mode = validate_input_mode(false, &source_ids, &[], &no_inputs, None)
        .expect("file readers with no streams should be Ok");
    assert_eq!(mode, OneshotInputMode::FileBased);
}

#[test]
fn validate_input_mode_file_based_with_streams_errors() {
    let source_ids = vec!["file_reader".to_string()];
    let inputs = vec![dummy_input("file_reader")];

    let result = validate_input_mode(false, &source_ids, &[], &inputs, None);
    let Err(StreamKitError::Configuration(msg)) = result else {
        panic!("expected Configuration error for file_reader + streams; got {result:?}");
    };
    assert!(
        msg.contains("Multipart streams were provided"),
        "error message should mention multipart streams: {msg}"
    );
}

#[test]
fn validate_input_mode_generator_ok() {
    let no_inputs: Vec<OneshotInput<DummyStream>> = Vec::new();

    let mode = validate_input_mode(false, &[], &[], &no_inputs, None)
        .expect("no sources + no streams should be Generator");
    assert_eq!(mode, OneshotInputMode::Generator);
}

#[test]
fn validate_input_mode_generator_with_streams_errors() {
    let inputs = vec![dummy_input("generator")];

    let result = validate_input_mode(false, &[], &[], &inputs, None);
    let Err(StreamKitError::Configuration(msg)) = result else {
        panic!("expected Configuration error for generator + streams; got {result:?}");
    };
    assert!(
        msg.contains("Multipart streams were provided"),
        "error message should mention multipart streams: {msg}"
    );
}

struct BinaryGeneratorNode {
    count: usize,
}

#[streamkit_core::async_trait]
impl ProcessorNode for BinaryGeneratorNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::One,
        }]
    }
    async fn run(self: Box<Self>, mut ctx: NodeContext) -> Result<(), StreamKitError> {
        while let Some(msg) = ctx.control_rx.recv().await {
            if matches!(msg, NodeControlMessage::Start) {
                break;
            }
        }
        for i in 0..self.count {
            let payload = format!("g{i}").into_bytes();
            let pkt = Packet::Binary {
                data: bytes::Bytes::from(payload),
                content_type: Some(std::borrow::Cow::Borrowed("application/octet-stream")),
                metadata: None,
            };
            let _ = ctx.output_sender.send("out", pkt).await;
        }
        Ok(())
    }
}

fn engine_with_registry(registry: streamkit_core::registry::NodeRegistry) -> Engine {
    Engine {
        registry: std::sync::Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: std::sync::Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: std::sync::Arc::new(streamkit_core::VideoFramePool::video_default()),
    }
}

#[tokio::test]
async fn run_oneshot_pipeline_generator_round_trip() {
    use streamkit_api::{Connection, EngineMode, Node, Pipeline};
    use streamkit_core::registry::NodeRegistry;

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::binary_generator",
        |_p| Ok(Box::new(BinaryGeneratorNode { count: 3 })),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let engine = engine_with_registry(registry);

    let mut nodes = indexmap::IndexMap::new();
    nodes.insert(
        "gen".to_string(),
        Node { kind: "test::binary_generator".to_string(), params: None, state: None },
    );
    nodes.insert(
        "sink".to_string(),
        Node {
            kind: "streamkit::http_output".to_string(),
            params: Some(serde_json::json!({ "content_type": "application/octet-stream" })),
            state: None,
        },
    );

    let definition = Pipeline {
        name: Some("gen-roundtrip".to_string()),
        description: None,
        mode: EngineMode::OneShot,
        client: None,
        nodes,
        connections: vec![Connection {
            from_node: "gen".to_string(),
            from_pin: "out".to_string(),
            to_node: "sink".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        }],
        view_data: None,
        runtime_schemas: None,
    };

    let cancel = tokio_util::sync::CancellationToken::new();
    let inputs: Vec<OneshotInput<DummyStream>> = Vec::new();
    let mut result = engine
        .run_oneshot_pipeline(
            definition,
            inputs,
            Some(OneshotEngineConfig::default()),
            Some(cancel),
        )
        .await
        .expect("generator pipeline should run end-to-end");

    assert_eq!(result.content_type, "application/octet-stream");

    let mut received = Vec::new();
    while let Ok(Some(chunk)) =
        tokio::time::timeout(std::time::Duration::from_secs(3), result.data_stream.recv()).await
    {
        received.extend_from_slice(&chunk);
    }
    let text = String::from_utf8(received).expect("output should be utf-8");
    assert!(
        text.contains("g0") && text.contains("g1") && text.contains("g2"),
        "expected all generator outputs in stream, got: {text:?}"
    );
}

#[tokio::test]
async fn run_oneshot_pipeline_propagates_validation_error() {
    use streamkit_api::{EngineMode, Node, Pipeline};
    use streamkit_core::registry::NodeRegistry;

    let registry = NodeRegistry::new();
    let engine = engine_with_registry(registry);

    let mut nodes = indexmap::IndexMap::new();
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
        connections: Vec::new(),
        view_data: None,
        runtime_schemas: None,
    };

    let inputs = vec![dummy_input("nonexistent")];
    let result = engine
        .run_oneshot_pipeline(
            definition,
            inputs,
            Some(OneshotEngineConfig::default()),
            Some(tokio_util::sync::CancellationToken::new()),
        )
        .await;
    assert!(
        matches!(result, Err(StreamKitError::Configuration(_))),
        "expected Configuration error when streams provided without http_input; got error: {:?}",
        result.err()
    );
}
