// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Tests for runtime param schema discovery in the dynamic engine.
//!
//! Exercises the `initialize_node` path where
//! `ProcessorNode::runtime_param_schema()` returns `Some(schema)`,
//! verifying that the schema is stored (via `get_runtime_schemas`) and
//! broadcast to subscribers (via `subscribe_runtime_schemas`).
//!
//! Also covers `node_added_not_emitted_on_creation_failure` and
//! `init_failure_transitions_to_failed`.

// Reason: tests use `.expect(...)` to surface helpful panic messages on
// setup failures (channel sends, subscription failures). No production code.
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};
use streamkit_core::control::EngineControlMessage;
use streamkit_core::error::StreamKitError;
use streamkit_core::node::ProcessorNode;
use streamkit_core::registry::NodeRegistry;
use streamkit_core::state::NodeState;
use streamkit_core::types::PacketType;

use crate::{DynamicEngineConfig, DynamicEngineHandle, Engine};

// ---------------------------------------------------------------------------
// Test node: returns a runtime param schema after initialization
// ---------------------------------------------------------------------------

struct SchemaNode;

#[streamkit_core::async_trait]
impl ProcessorNode for SchemaNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        vec![streamkit_core::OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::Binary,
            cardinality: streamkit_core::PinCardinality::Broadcast,
        }]
    }

    fn runtime_param_schema(&self) -> Option<serde_json::Value> {
        Some(serde_json::json!({
            "type": "object",
            "properties": {
                "rate": { "type": "number" }
            }
        }))
    }

    async fn run(
        self: Box<Self>,
        mut ctx: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        loop {
            match ctx.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(
                    streamkit_core::control::NodeControlMessage::Start
                    | streamkit_core::control::NodeControlMessage::UpdateParams(_),
                ) => {},
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test node: constructor succeeds but initialize() fails
// ---------------------------------------------------------------------------

struct FailingInitNode;

#[streamkit_core::async_trait]
impl ProcessorNode for FailingInitNode {
    fn input_pins(&self) -> Vec<streamkit_core::InputPin> {
        Vec::new()
    }
    fn output_pins(&self) -> Vec<streamkit_core::OutputPin> {
        Vec::new()
    }

    async fn initialize(
        &mut self,
        _ctx: &streamkit_core::InitContext,
    ) -> Result<streamkit_core::pins::PinUpdate, StreamKitError> {
        Err(StreamKitError::Runtime("init probe failed: device unavailable".to_string()))
    }

    async fn run(self: Box<Self>, _ctx: streamkit_core::NodeContext) -> Result<(), StreamKitError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_handle_with_schema_and_failing() -> DynamicEngineHandle {
    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::schema",
        |_p| Ok(Box::new(SchemaNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::failing_init",
        |_p| Ok(Box::new(FailingInitNode) as Box<dyn ProcessorNode>),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    registry.register_dynamic(
        "test::failing_ctor",
        |_p| Err(StreamKitError::Runtime("ctor boom".to_string())),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );
    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
    };
    engine.start_dynamic_actor(DynamicEngineConfig::default())
}

async fn wait_for_node_state(
    handle: &DynamicEngineHandle,
    node_id: &str,
    pred: impl Fn(&NodeState) -> bool,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(states) = handle.get_node_states().await {
            if let Some(state) = states.get(node_id) {
                if pred(state) {
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("node '{node_id}' did not reach expected state within {timeout:?}");
}

// ---------------------------------------------------------------------------
// Test 4: Runtime schema discovery
// ---------------------------------------------------------------------------

#[tokio::test]
async fn runtime_schema_discovery_reaches_get_and_subscribe() {
    let handle = build_handle_with_schema_and_failing();

    let mut schema_rx = handle.subscribe_runtime_schemas().await.expect("subscribe");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "s1".to_string(),
            kind: "test::schema".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    wait_for_node_state(
        &handle,
        "s1",
        |s| !matches!(s, NodeState::Creating),
        Duration::from_secs(3),
    )
    .await;

    // Verify via subscriber.
    let notif = tokio::time::timeout(Duration::from_secs(3), schema_rx.recv())
        .await
        .expect("schema notification should arrive")
        .expect("subscriber channel still open");

    assert_eq!(notif.node_id, "s1");
    assert_eq!(notif.schema["properties"]["rate"]["type"], "number");

    // Verify via getter.
    let schemas = handle.get_runtime_schemas().await.expect("get_runtime_schemas");
    let schema = schemas.get("s1").expect("schema should be stored for s1");
    assert_eq!(schema["properties"]["rate"]["type"], "number");

    handle.shutdown_and_wait().await.expect("shutdown");
}

// ---------------------------------------------------------------------------
// Test 6: No NodeAddedNotification on creation failure
// ---------------------------------------------------------------------------

#[tokio::test]
async fn node_added_not_emitted_on_creation_failure() {
    let handle = build_handle_with_schema_and_failing();

    let mut added_rx = handle.subscribe_node_added().await.expect("subscribe");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "bad".to_string(),
            kind: "test::failing_ctor".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    // Wait for the node to transition to Failed.
    wait_for_node_state(
        &handle,
        "bad",
        |s| matches!(s, NodeState::Failed { .. }),
        Duration::from_secs(3),
    )
    .await;

    // Allow a short settling period, then verify no notification arrived.
    tokio::time::sleep(Duration::from_millis(100)).await;

    assert!(
        added_rx.try_recv().is_err(),
        "NodeAddedNotification must NOT be sent for a failed creation"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}

// ---------------------------------------------------------------------------
// Test 9: Init failure transitions node to Failed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn init_failure_transitions_to_failed() {
    let handle = build_handle_with_schema_and_failing();

    let mut added_rx = handle.subscribe_node_added().await.expect("subscribe");

    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "init_fail".to_string(),
            kind: "test::failing_init".to_string(),
            params: None,
        })
        .await
        .expect("send AddNode");

    wait_for_node_state(
        &handle,
        "init_fail",
        |s| matches!(s, NodeState::Failed { .. }),
        Duration::from_secs(3),
    )
    .await;

    let states = handle.get_node_states().await.expect("get states");
    match states.get("init_fail") {
        Some(NodeState::Failed { reason }) => {
            assert!(
                reason.contains("device unavailable"),
                "failure reason should propagate from initialize(); got: {reason}"
            );
        },
        other => panic!("expected Failed state, got {other:?}"),
    }

    // No NodeAddedNotification for init failures either.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        added_rx.try_recv().is_err(),
        "NodeAddedNotification must NOT be sent when initialize() fails"
    );

    handle.shutdown_and_wait().await.expect("shutdown");
}
