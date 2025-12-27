// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

//! Test utilities for node testing

use std::collections::HashMap;
use streamkit_core::node::{NodeContext, OutputRouting, OutputSender, RoutedPacketMessage};
use streamkit_core::state::NodeStateUpdate;
use streamkit_core::types::Packet;
use tokio::sync::mpsc;

/// Creates a test NodeContext with mock channels
#[allow(clippy::implicit_hasher)]
pub fn create_test_context(
    inputs: HashMap<String, mpsc::Receiver<streamkit_core::types::Packet>>,
    batch_size: usize,
) -> (NodeContext, MockOutputSender, mpsc::Receiver<NodeStateUpdate>) {
    let (_control_tx, control_rx) = mpsc::channel(10);
    let (state_tx, state_rx) = mpsc::channel(10);
    let (stats_tx, _stats_rx) = mpsc::channel(10);
    let (pin_mgmt_tx, pin_mgmt_rx) = mpsc::channel(10);
    // Drop the sender so nodes using this context don't wait for pin management messages
    drop(pin_mgmt_tx);

    let mock_sender = MockOutputSender::new();
    let output_sender = mock_sender.to_output_sender("test_node".to_string());

    let context = NodeContext {
        inputs,
        control_rx,
        output_sender,
        batch_size,
        state_tx,
        stats_tx: Some(stats_tx),
        telemetry_tx: None, // Test contexts don't emit telemetry
        session_id: None,   // Test contexts don't have sessions
        cancellation_token: None,
        pin_management_rx: Some(pin_mgmt_rx), // Provide channel for dynamic pins support
        audio_pool: None,
    };

    (context, mock_sender, state_rx)
}

/// Mock OutputSender that captures sent packets via a channel.
/// Uses the same RoutedPacketMessage type as the real implementation for consistency.
#[derive(Clone)]
pub struct MockOutputSender {
    receiver: std::sync::Arc<tokio::sync::Mutex<mpsc::Receiver<RoutedPacketMessage>>>,
    sender: mpsc::Sender<RoutedPacketMessage>,
}

impl Default for MockOutputSender {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel(1000); // Increased from 100 to handle large test files
        Self { receiver: std::sync::Arc::new(tokio::sync::Mutex::new(receiver)), sender }
    }
}

impl MockOutputSender {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an OutputSender from this mock
    pub fn to_output_sender(&self, node_name: String) -> OutputSender {
        OutputSender::new(node_name, OutputRouting::Routed(self.sender.clone()))
    }

    /// Receive a single packet (non-blocking).
    /// Returns (node_name, pin_name, packet) as Strings for test convenience.
    pub async fn try_recv(&self) -> Option<(String, String, Packet)> {
        let mut receiver = self.receiver.lock().await;
        receiver
            .try_recv()
            .ok()
            .map(|(node, pin, packet)| (node.to_string(), pin.to_string(), packet))
    }

    /// Receive packets with timeout.
    /// Returns (node_name, pin_name, packet) as Strings for test convenience.
    pub async fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Option<(String, String, Packet)> {
        let mut receiver = self.receiver.lock().await;
        tokio::time::timeout(timeout, receiver.recv())
            .await
            .ok()
            .flatten()
            .map(|(node, pin, packet)| (node.to_string(), pin.to_string(), packet))
    }

    /// Collect all available packets.
    /// Returns Vec of (node_name, pin_name, packet) as Strings for test convenience.
    pub async fn collect_packets(&self) -> Vec<(String, String, Packet)> {
        let mut packets = Vec::new();
        while let Some(packet) = self.try_recv().await {
            packets.push(packet);
        }
        packets
    }

    /// Get all packets sent to a specific output pin
    pub async fn get_packets_for_pin(&self, pin_name: &str) -> Vec<streamkit_core::types::Packet> {
        let all_packets = self.collect_packets().await;
        all_packets
            .into_iter()
            .filter(|(_, pin, _)| pin == pin_name)
            .map(|(_, _, packet)| packet)
            .collect()
    }
}

/// Helper to create a simple audio packet for testing
pub fn create_test_audio_packet(
    sample_rate: u32,
    channels: u16,
    samples_per_channel: usize,
    fill_value: f32,
) -> Packet {
    let mut samples = Vec::with_capacity(samples_per_channel * channels as usize);
    for _ in 0..(samples_per_channel * channels as usize) {
        samples.push(fill_value);
    }

    Packet::Audio(streamkit_core::types::AudioFrame::new(sample_rate, channels, samples))
}

/// Helper to create a test binary packet
pub fn create_test_binary_packet(data: Vec<u8>) -> Packet {
    Packet::Binary { data: bytes::Bytes::from(data), content_type: None, metadata: None }
}

/// Helper to extract audio data from a packet
pub fn extract_audio_data(packet: &Packet) -> Option<&[f32]> {
    match packet {
        Packet::Audio(frame) => Some(&frame.samples),
        _ => None,
    }
}

/// Helper to assert that a state update was received and matches the expected state type
///
/// # Panics
///
/// Panics if the state update is not received within the timeout or if the state does not match the expected state.
#[allow(clippy::expect_used)]
pub async fn assert_state_update(
    state_rx: &mut mpsc::Receiver<NodeStateUpdate>,
    expected_state_matcher: impl Fn(&streamkit_core::NodeState) -> bool,
    description: &str,
) {
    let update = tokio::time::timeout(std::time::Duration::from_secs(20), state_rx.recv())
        .await
        .expect("Timeout waiting for state update")
        .expect("State channel closed");

    assert!(
        expected_state_matcher(&update.state),
        "Unexpected state update: {:?}. Expected: {}",
        update.state,
        description
    );
}

/// Helper to assert Initializing state
pub async fn assert_state_initializing(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(
        state_rx,
        |s| matches!(s, streamkit_core::NodeState::Initializing),
        "Initializing",
    )
    .await;
}

/// Helper to assert Running state
pub async fn assert_state_running(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(state_rx, |s| matches!(s, streamkit_core::NodeState::Running), "Running")
        .await;
}

/// Helper to assert Stopped state
pub async fn assert_state_stopped(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(
        state_rx,
        |s| matches!(s, streamkit_core::NodeState::Stopped { .. }),
        "Stopped",
    )
    .await;
}

/// Helper to assert Failed state
pub async fn assert_state_failed(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(
        state_rx,
        |s| matches!(s, streamkit_core::NodeState::Failed { .. }),
        "Failed",
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::float_cmp)]
    fn test_create_test_audio_packet() {
        let packet = create_test_audio_packet(48000, 2, 480, 1.0);

        match packet {
            Packet::Audio(frame) => {
                assert_eq!(frame.sample_rate, 48000);
                assert_eq!(frame.channels, 2);
                assert_eq!(frame.samples.len(), 960); // 480 * 2
                assert_eq!(frame.samples[0], 1.0);
            },
            _ => panic!("Expected audio packet"),
        }
    }

    #[test]
    #[allow(clippy::expect_used, clippy::float_cmp)]
    fn test_extract_audio_data() {
        let packet = create_test_audio_packet(48000, 2, 480, 0.75);
        let data = extract_audio_data(&packet).expect("Should have audio data");
        assert_eq!(data.len(), 960);
        assert_eq!(data[0], 0.75);
    }
}
