// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::super::*;
use crate::constants::{
    DEFAULT_BATCH_SIZE, DEFAULT_ONESHOT_IO_CAPACITY, DEFAULT_ONESHOT_MEDIA_CAPACITY,
};
use crate::oneshot::OneshotEngineConfig;
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
