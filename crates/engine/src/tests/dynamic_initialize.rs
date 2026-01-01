// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use super::super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use streamkit_core::{NodeRegistry, ProcessorNode, StreamKitError};

struct InitCalledNode {
    initialize_called: Arc<AtomicBool>,
}

#[streamkit_core::async_trait]
impl ProcessorNode for InitCalledNode {
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
        self.initialize_called.store(true, Ordering::SeqCst);
        Ok(streamkit_core::pins::PinUpdate::NoChange)
    }

    async fn run(
        self: Box<Self>,
        mut context: streamkit_core::NodeContext,
    ) -> Result<(), StreamKitError> {
        // Stay alive until shutdown so the engine keeps it as a live node.
        loop {
            match context.control_rx.recv().await {
                Some(streamkit_core::control::NodeControlMessage::Shutdown) | None => return Ok(()),
                Some(
                    streamkit_core::control::NodeControlMessage::Start
                    | streamkit_core::control::NodeControlMessage::UpdateParams(_),
                ) => {},
            }
        }
    }
}

#[tokio::test]
async fn test_dynamic_engine_calls_initialize() {
    let initialize_called = Arc::new(AtomicBool::new(false));
    let initialize_called_clone = initialize_called.clone();

    let mut registry = NodeRegistry::new();
    registry.register_dynamic(
        "test::init_called",
        move |_params| {
            Ok(Box::new(InitCalledNode { initialize_called: initialize_called_clone.clone() }))
        },
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
    };
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());

    if let Err(e) = handle
        .send_control(streamkit_core::control::EngineControlMessage::AddNode {
            node_id: "n1".to_string(),
            kind: "test::init_called".to_string(),
            params: None,
        })
        .await
    {
        panic!("failed to add node: {e}");
    }

    // Wait up to 1s for initialize to be called.
    let mut waited = 0u32;
    while !initialize_called.load(Ordering::SeqCst) && waited < 100 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        waited += 1;
    }

    assert!(
        initialize_called.load(Ordering::SeqCst),
        "expected ProcessorNode::initialize to be called for dynamic engine nodes"
    );

    if let Err(e) = handle.shutdown_and_wait().await {
        panic!("failed to shutdown engine: {e}");
    }
}
