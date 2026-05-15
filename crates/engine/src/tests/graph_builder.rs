// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::super::*;
use crate::constants::DEFAULT_ONESHOT_MEDIA_CAPACITY;
use streamkit_core::types::{Packet, PacketType};
use streamkit_core::{
    InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

struct SourceNode;

#[streamkit_core::async_trait]
impl ProcessorNode for SourceNode {
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
        let _ = ctx.output_sender.send("out", Packet::Text("hello".into())).await;
        Ok(())
    }
}

struct SinkNode;

#[streamkit_core::async_trait]
impl ProcessorNode for SinkNode {
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
        while rx.recv().await.is_some() {}
        Ok(())
    }
}

struct BinarySourceNode;

#[streamkit_core::async_trait]
impl ProcessorNode for BinarySourceNode {
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
    async fn run(self: Box<Self>, _ctx: NodeContext) -> Result<(), StreamKitError> {
        Ok(())
    }
}

struct PassthroughNode;

#[streamkit_core::async_trait]
impl ProcessorNode for PassthroughNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }
    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::One,
        }]
    }
    async fn run(self: Box<Self>, _ctx: NodeContext) -> Result<(), StreamKitError> {
        Ok(())
    }
}

struct StandaloneNode;

#[streamkit_core::async_trait]
impl ProcessorNode for StandaloneNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<OutputPin> {
        Vec::new()
    }
    async fn run(self: Box<Self>, _ctx: NodeContext) -> Result<(), StreamKitError> {
        Ok(())
    }
}

fn conn(from_node: &str, from_pin: &str, to_node: &str, to_pin: &str) -> Connection {
    Connection {
        from_node: from_node.to_string(),
        from_pin: from_pin.to_string(),
        to_node: to_node.to_string(),
        to_pin: to_pin.to_string(),
        mode: streamkit_api::ConnectionMode::Reliable,
    }
}

fn kind(name: &str) -> (String, String) {
    (name.to_string(), format!("test::{name}"))
}

async fn wire(
    nodes: HashMap<String, Box<dyn ProcessorNode>>,
    connections: &[Connection],
    node_kinds: &HashMap<String, String>,
) -> Result<HashMap<String, graph_builder::LiveNode>, StreamKitError> {
    graph_builder::wire_and_spawn_graph(
        nodes,
        connections,
        node_kinds,
        1,
        DEFAULT_ONESHOT_MEDIA_CAPACITY,
        None,
        None,
        None,
        None,
        None,
    )
    .await
}

async fn drain(live: HashMap<String, graph_builder::LiveNode>) {
    let handles: Vec<_> = live.into_values().map(|n| n.task_handle).collect();
    let _ =
        tokio::time::timeout(std::time::Duration::from_secs(5), futures::future::join_all(handles))
            .await;
}

#[tokio::test]
async fn linear_pipeline_spawns_and_connects() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("src".to_string(), Box::new(SourceNode));
    nodes.insert("sink".to_string(), Box::new(SinkNode));

    let connections = vec![conn("src", "out", "sink", "in")];
    let node_kinds: HashMap<String, String> = [kind("src"), kind("sink")].into_iter().collect();

    let Ok(live) = wire(nodes, &connections, &node_kinds).await else {
        panic!("wiring a simple linear pipeline should succeed");
    };

    assert_eq!(live.len(), 2);
    assert!(live.contains_key("src"));
    assert!(live.contains_key("sink"));
    drain(live).await;
}

#[tokio::test]
async fn type_incompatible_connection_rejected() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("src".to_string(), Box::new(BinarySourceNode));
    nodes.insert("sink".to_string(), Box::new(SinkNode));

    let connections = vec![conn("src", "out", "sink", "in")];
    let node_kinds: HashMap<String, String> = [kind("src"), kind("sink")].into_iter().collect();

    let Err(err) = wire(nodes, &connections, &node_kinds).await else {
        panic!("Binary -> Text connection should be rejected");
    };

    match err {
        StreamKitError::Configuration(msg) => {
            assert!(msg.contains("Incompatible"), "error should mention incompatibility: {msg}");
        },
        other => panic!("expected Configuration error, got: {other:?}"),
    }
}

#[tokio::test]
async fn passthrough_wiring_resolves_type_from_upstream() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("src".to_string(), Box::new(SourceNode));
    nodes.insert("pass".to_string(), Box::new(PassthroughNode));
    nodes.insert("sink".to_string(), Box::new(SinkNode));

    let connections = vec![conn("src", "out", "pass", "in"), conn("pass", "out", "sink", "in")];
    let node_kinds: HashMap<String, String> =
        [kind("src"), kind("pass"), kind("sink")].into_iter().collect();

    let Ok(live) = wire(nodes, &connections, &node_kinds).await else {
        panic!("passthrough type resolution should succeed");
    };

    assert_eq!(live.len(), 3);
    drain(live).await;
}

#[tokio::test]
async fn missing_output_pin_name_produces_error() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("src".to_string(), Box::new(SourceNode));
    nodes.insert("sink".to_string(), Box::new(SinkNode));

    let connections = vec![conn("src", "nonexistent", "sink", "in")];
    let node_kinds: HashMap<String, String> = [kind("src"), kind("sink")].into_iter().collect();

    let Err(err) = wire(nodes, &connections, &node_kinds).await else {
        panic!("referencing a nonexistent output pin should fail");
    };

    match err {
        StreamKitError::Configuration(msg) => {
            assert!(msg.contains("Unknown output pin"), "error should mention unknown pin: {msg}");
        },
        other => panic!("expected Configuration error, got: {other:?}"),
    }
}

#[tokio::test]
async fn missing_input_pin_name_produces_error() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("src".to_string(), Box::new(SourceNode));
    nodes.insert("sink".to_string(), Box::new(SinkNode));

    let connections = vec![conn("src", "out", "sink", "nonexistent")];
    let node_kinds: HashMap<String, String> = [kind("src"), kind("sink")].into_iter().collect();

    let Err(err) = wire(nodes, &connections, &node_kinds).await else {
        panic!("referencing a nonexistent input pin should fail");
    };

    match err {
        StreamKitError::Configuration(msg) => {
            assert!(msg.contains("Unknown input pin"), "error should mention unknown pin: {msg}");
        },
        other => panic!("expected Configuration error, got: {other:?}"),
    }
}

#[tokio::test]
async fn standalone_node_runs_without_connections() {
    let mut nodes: HashMap<String, Box<dyn ProcessorNode>> = HashMap::new();
    nodes.insert("alone".to_string(), Box::new(StandaloneNode));

    let connections: Vec<Connection> = vec![];
    let node_kinds: HashMap<String, String> = std::iter::once(kind("alone")).collect();

    let Ok(live) = wire(nodes, &connections, &node_kinds).await else {
        panic!("standalone node should spawn successfully");
    };

    assert_eq!(live.len(), 1);
    assert!(live.contains_key("alone"));

    let handles: Vec<_> = live.into_values().map(|n| n.task_handle).collect();
    let results =
        tokio::time::timeout(std::time::Duration::from_secs(5), futures::future::join_all(handles))
            .await;
    assert!(results.is_ok(), "standalone node should complete");
}
