// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use async_trait::async_trait;
use streamkit_core::types::PacketType;
use streamkit_core::{
    state_helpers, InputPin, NodeContext, OutputPin, PinCardinality, ProcessorNode, StreamKitError,
};

/// A simple node that does nothing, just passes any packet it receives through.
/// This is useful for testing the pipeline architecture and routing.
#[derive(Default)]
pub struct PassthroughNode;

#[async_trait]
impl ProcessorNode for PassthroughNode {
    fn input_pins(&self) -> Vec<InputPin> {
        vec![InputPin {
            name: "in".to_string(),
            // This node accepts any packet type.
            accepts_types: vec![PacketType::Any],
            cardinality: PinCardinality::One,
        }]
    }

    fn output_pins(&self) -> Vec<OutputPin> {
        vec![OutputPin {
            name: "out".to_string(),
            // It outputs whatever it receives, using passthrough type inference
            produces_type: PacketType::Passthrough,
            cardinality: PinCardinality::Broadcast,
        }]
    }

    async fn run(self: Box<Self>, mut context: NodeContext) -> Result<(), StreamKitError> {
        let node_name = context.output_sender.node_name().to_string();
        state_helpers::emit_initializing(&context.state_tx, &node_name);

        tracing::info!("PassthroughNode starting");
        let mut input_rx = context.take_input("in")?;

        state_helpers::emit_running(&context.state_tx, &node_name);

        // This node doesn't have tunable parameters, so we can ignore the control channel.
        // The loop will simply exit when the input channel is closed.
        while let Some(packet) = context.recv_with_cancellation(&mut input_rx).await {
            // Forward the packet directly to the "out" pin.
            if context.output_sender.send("out", packet).await.is_err() {
                tracing::debug!("Output channel closed, stopping node");
                break;
            }
        }

        state_helpers::emit_stopped(&context.state_tx, &node_name, "input_closed");

        tracing::info!("PassthroughNode shutting down.");
        Ok(())
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::used_underscore_binding
)]
mod tests {
    use super::*;
    use crate::test_utils::{
        assert_state_initializing, assert_state_running, assert_state_stopped,
        create_test_audio_packet, create_test_binary_packet, create_test_context,
        extract_audio_data,
    };
    use std::collections::HashMap;
    use streamkit_core::types::Packet;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_passthrough_audio() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PassthroughNode;
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send audio packet
        let audio_packet = create_test_audio_packet(48000, 2, 100, 0.75);
        input_tx.send(audio_packet.clone()).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify packet passed through unchanged
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        let audio_data = extract_audio_data(&output_packets[0]).expect("Should be audio");
        assert_eq!(audio_data.len(), 200); // 100 * 2 channels
        assert_eq!(audio_data[0], 0.75);
    }

    #[tokio::test]
    async fn test_passthrough_binary() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PassthroughNode;
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send binary packet
        let test_data = vec![1, 2, 3, 4, 5];
        let binary_packet = create_test_binary_packet(test_data.clone());
        input_tx.send(binary_packet).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify packet passed through
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        match &output_packets[0] {
            Packet::Binary { data, .. } => {
                assert_eq!(data.as_ref(), test_data.as_slice());
            },
            _ => panic!("Expected binary packet"),
        }
    }

    #[tokio::test]
    async fn test_passthrough_text() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PassthroughNode;
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send text packet
        let text = "Hello, StreamKit!".to_string();
        input_tx.send(Packet::Text(text.clone().into())).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify packet passed through
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 1);

        match &output_packets[0] {
            Packet::Text(s) => {
                assert_eq!(s.as_ref(), text.as_str());
            },
            _ => panic!("Expected text packet"),
        }
    }

    #[tokio::test]
    async fn test_passthrough_multiple_packets() {
        let (input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PassthroughNode;
        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;

        // Send mixed packet types
        input_tx.send(create_test_audio_packet(48000, 2, 10, 1.0)).await.unwrap();
        input_tx.send(Packet::Text("test".into())).await.unwrap();
        input_tx.send(create_test_binary_packet(vec![0xFF])).await.unwrap();

        drop(input_tx);
        assert_state_stopped(&mut state_rx).await;
        node_handle.await.unwrap().unwrap();

        // Verify all packets passed through
        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 3);

        // Verify packet types match
        assert!(matches!(output_packets[0], Packet::Audio(_)));
        assert!(matches!(output_packets[1], Packet::Text(_)));
        assert!(matches!(output_packets[2], Packet::Binary { .. }));
    }

    #[tokio::test]
    async fn test_passthrough_empty_input() {
        let (_input_tx, input_rx) = mpsc::channel(10);
        let mut inputs = HashMap::new();
        inputs.insert("in".to_string(), input_rx);

        let (context, mock_sender, mut state_rx) = create_test_context(inputs, 10);

        let node = PassthroughNode;

        drop(_input_tx);

        let node_handle = tokio::spawn(async move { Box::new(node).run(context).await });

        assert_state_initializing(&mut state_rx).await;
        assert_state_running(&mut state_rx).await;
        assert_state_stopped(&mut state_rx).await;

        node_handle.await.unwrap().unwrap();

        let output_packets = mock_sender.get_packets_for_pin("out").await;
        assert_eq!(output_packets.len(), 0);
    }
}
