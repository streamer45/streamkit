// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::super::*;
use crate::constants::DEFAULT_ONESHOT_MEDIA_CAPACITY;
use streamkit_core::types::PacketType;
use streamkit_core::{
    InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

struct NoopNode;

#[streamkit_core::async_trait]
impl ProcessorNode for NoopNode {
    fn input_pins(&self) -> Vec<InputPin> {
        Vec::new()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, _context: NodeContext) -> Result<(), StreamKitError> {
        Ok(())
    }
}

#[tokio::test]
async fn test_oneshot_rejects_fanout() {
    let mut nodes: std::collections::HashMap<String, Box<dyn ProcessorNode>> =
        std::collections::HashMap::new();
    nodes.insert("src".to_string(), Box::new(NoopNode));
    nodes.insert("a".to_string(), Box::new(NoopNode));
    nodes.insert("b".to_string(), Box::new(NoopNode));

    let connections = vec![
        Connection {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "a".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
        Connection {
            from_node: "src".to_string(),
            from_pin: "out".to_string(),
            to_node: "b".to_string(),
            to_pin: "in".to_string(),
            mode: streamkit_api::ConnectionMode::Reliable,
        },
    ];

    let node_kinds = [
        ("src".to_string(), "test::noop".to_string()),
        ("a".to_string(), "test::noop".to_string()),
        ("b".to_string(), "test::noop".to_string()),
    ]
    .into_iter()
    .collect();

    let Err(err) = graph_builder::wire_and_spawn_graph(
        nodes,
        &connections,
        &node_kinds,
        1,
        DEFAULT_ONESHOT_MEDIA_CAPACITY,
        None,
        None,
        None,
    )
    .await
    else {
        panic!("expected fan-out to be rejected in oneshot graph builder");
    };

    match err {
        StreamKitError::Configuration(msg) => assert!(msg.contains("fan-out not supported yet")),
        other => panic!("expected configuration error, got: {other:?}"),
    }
}
