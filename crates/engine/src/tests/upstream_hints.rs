// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Integration test for upstream resize hints.
//!
//! Validates the full data-flow: compositor receives an `UpdateParams` that
//! changes a layer rect → compositor sends `UpstreamHint::PreferredSize` via
//! the hint channel → source node receives the hint through the engine wiring.

use super::super::*;
use std::sync::{Arc, Mutex};
use streamkit_core::control::{ConnectionMode, EngineControlMessage, NodeControlMessage};
use streamkit_core::pins::PinManagementMessage;
use streamkit_core::types::{PacketType, PixelFormat, RawVideoFormat};
use streamkit_core::{
    InputPin, NodeContext, NodeRegistry, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
    UpstreamHint,
};
use tokio::sync::mpsc;

/// Captured hint for test assertions.
#[derive(Debug, Clone)]
struct CapturedHint {
    width: u32,
    height: u32,
}

/// A minimal test source node that captures upstream hints received via
/// `OutputHintChannel` pin management messages.
struct HintCapturingSource {
    captured_hints: Arc<Mutex<Vec<CapturedHint>>>,
}

impl HintCapturingSource {
    /// Drain all hint receivers, recording any PreferredSize hints.
    #[allow(clippy::expect_used)]
    fn drain_hints(
        hint_receivers: &mut Vec<mpsc::Receiver<UpstreamHint>>,
        captured: &Arc<Mutex<Vec<CapturedHint>>>,
    ) {
        hint_receivers.retain_mut(|rx| loop {
            match rx.try_recv() {
                Ok(UpstreamHint::PreferredSize { width, height }) => {
                    captured.lock().expect("lock poisoned").push(CapturedHint { width, height });
                },
                Ok(_) => {}, // future variants
                Err(mpsc::error::TryRecvError::Empty) => return true,
                Err(mpsc::error::TryRecvError::Disconnected) => return false,
            }
        });
    }
}

#[streamkit_core::async_trait]
impl ProcessorNode for HintCapturingSource {
    fn input_pins(&self) -> Vec<InputPin> {
        Vec::new()
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            produces_type: PacketType::RawVideo(RawVideoFormat {
                width: None,
                height: None,
                pixel_format: PixelFormat::Rgba8,
            }),
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let mut hint_receivers: Vec<mpsc::Receiver<UpstreamHint>> = Vec::new();

        // Use a short poll interval to drain hints promptly.
        let mut poll_tick = tokio::time::interval(std::time::Duration::from_millis(10));
        poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Some(ctrl) = context.control_rx.recv() => {
                    match ctrl {
                        NodeControlMessage::Shutdown => return Ok(()),
                        NodeControlMessage::Start | NodeControlMessage::UpdateParams(_) => {},
                    }
                }
                Some(msg) = async {
                    match context.pin_management_rx {
                        Some(ref mut rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let PinManagementMessage::OutputHintChannel { hint_rx, .. } = msg {
                        hint_receivers.push(hint_rx);
                    }
                }
                _ = poll_tick.tick() => {
                    Self::drain_hints(&mut hint_receivers, &self.captured_hints);
                }
            }
        }
    }
}

/// Poll `captured_hints` until `predicate` returns `true` or `timeout` elapses.
/// Returns the snapshot of hints when the predicate first matches.
/// Panics with `msg` if the timeout is reached.
#[allow(clippy::expect_used)]
async fn poll_hints(
    captured_hints: &Arc<Mutex<Vec<CapturedHint>>>,
    predicate: impl Fn(&[CapturedHint]) -> bool,
    timeout: std::time::Duration,
    msg: &str,
) -> Vec<CapturedHint> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(20));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let snapshot = captured_hints.lock().expect("lock poisoned").clone();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(tokio::time::Instant::now() < deadline, "{msg}");
    }
}

/// Full engine-level test: source → compositor with dynamic pin.
/// After connecting, update the compositor's layer rect and verify the
/// source received the resize hint.
#[tokio::test]
#[allow(clippy::expect_used)]
async fn test_upstream_resize_hint_end_to_end() {
    let captured_hints = Arc::new(Mutex::new(Vec::<CapturedHint>::new()));
    let captured_clone = captured_hints.clone();

    let mut registry = NodeRegistry::new();

    // Register the test source.
    registry.register_dynamic(
        "test::hint_source",
        move |_params| Ok(Box::new(HintCapturingSource { captured_hints: captured_clone.clone() })),
        serde_json::json!({}),
        vec!["test".to_string()],
        false,
    );

    // Register the real compositor node.
    let constraints = streamkit_core::constraints::GlobalNodeConstraints::default();
    streamkit_nodes::video::compositor::register_compositor_nodes(&mut registry, &constraints);

    let engine = Engine {
        registry: Arc::new(std::sync::RwLock::new(registry)),
        audio_pool: Arc::new(streamkit_core::AudioFramePool::audio_default()),
        video_pool: Arc::new(streamkit_core::VideoFramePool::video_default()),
    };
    let handle = engine.start_dynamic_actor(DynamicEngineConfig::default());

    // 1. Add the source node.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "source".to_string(),
            kind: "test::hint_source".to_string(),
            params: None,
        })
        .await
        .expect("add source");

    // 2. Add the compositor node with an initial layer config.
    handle
        .send_control(EngineControlMessage::AddNode {
            node_id: "compositor".to_string(),
            kind: "video::compositor".to_string(),
            params: Some(serde_json::json!({
                "width": 1280,
                "height": 720,
                "fps": 30,
                "layers": {
                    "in_cam": {
                        "rect": { "x": 0, "y": 0, "width": 640, "height": 480 }
                    }
                }
            })),
        })
        .await
        .expect("add compositor");

    // 3. Connect source → compositor (this triggers the hint channel wiring).
    //    Small delay to let both nodes start their run loops.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    handle
        .send_control(EngineControlMessage::Connect {
            from_node: "source".to_string(),
            from_pin: "out".to_string(),
            to_node: "compositor".to_string(),
            to_pin: "in_cam".to_string(),
            mode: ConnectionMode::Reliable,
        })
        .await
        .expect("connect");

    // Poll for the initial hint from the connection (the compositor sends
    // a hint on AddedInputPin if the layer rect is already configured).
    let initial_hints = poll_hints(
        &captured_hints,
        |h| !h.is_empty(),
        std::time::Duration::from_secs(5),
        "source should receive an initial resize hint on connection (timed out)",
    )
    .await;

    assert_eq!(initial_hints[0].width, 640);
    assert_eq!(initial_hints[0].height, 480);

    // 4. Update the compositor's layer rect to a new size.
    captured_hints.lock().expect("lock poisoned").clear();

    handle
        .send_control(EngineControlMessage::TuneNode {
            node_id: "compositor".to_string(),
            message: NodeControlMessage::UpdateParams(serde_json::json!({
                "layers": {
                    "in_cam": {
                        "rect": { "x": 0, "y": 0, "width": 1280, "height": 720 }
                    }
                }
            })),
        })
        .await
        .expect("update params");

    // Poll for the resize hint to propagate.
    let resize_hints = poll_hints(
        &captured_hints,
        |h| !h.is_empty(),
        std::time::Duration::from_secs(5),
        "source should receive a resize hint after UpdateParams (timed out)",
    )
    .await;

    assert_eq!(resize_hints[0].width, 1280, "hint width should match new layer rect");
    assert_eq!(resize_hints[0].height, 720, "hint height should match new layer rect");

    // Shutdown.
    handle.shutdown_and_wait().await.expect("shutdown");
}
