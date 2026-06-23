// SPDX-FileCopyrightText: © 2025 StreamKit Contributors
//
// SPDX-License-Identifier: MPL-2.0

use std::collections::HashMap;
use std::path::PathBuf;
use streamkit_core::node::{NodeContext, OutputRouting, OutputSender, RoutedPacketMessage};
use streamkit_core::state::NodeStateUpdate;
use streamkit_core::types::{Packet, PixelFormat, VideoFrame, VideoLayout};
use tokio::sync::mpsc;

pub fn test_asset_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[allow(clippy::implicit_hasher)]
pub fn create_test_context(
    inputs: HashMap<String, mpsc::Receiver<streamkit_core::types::Packet>>,
    batch_size: usize,
) -> (NodeContext, MockOutputSender, mpsc::Receiver<NodeStateUpdate>) {
    let (_control_tx, control_rx) = mpsc::channel(10);
    let (state_tx, state_rx) = mpsc::channel(10);
    let state_tx = streamkit_core::state::NodeStateSender::new(state_tx, 0);
    let (stats_tx, _stats_rx) = mpsc::channel(10);
    let (pin_mgmt_tx, pin_mgmt_rx) = mpsc::channel(10);
    drop(pin_mgmt_tx);

    let mock_sender = MockOutputSender::new();
    let output_sender = mock_sender.to_output_sender("test_node".to_string());

    let context = NodeContext {
        inputs,
        input_types: HashMap::new(),
        control_rx,
        output_sender,
        batch_size,
        state_tx,
        stats_tx: Some(stats_tx),
        telemetry_tx: None,
        session_id: None,
        cancellation_token: None,
        pin_management_rx: Some(pin_mgmt_rx), // Provide channel for dynamic pins support
        audio_pool: None,
        video_pool: None,
        pipeline_mode: streamkit_core::node::PipelineMode::Dynamic,
        view_data_tx: None,
        engine_control_tx: None,
        asset_root: test_asset_root(),
    };

    (context, mock_sender, state_rx)
}

#[allow(clippy::implicit_hasher)]
pub fn create_test_context_with_asset_root(
    inputs: HashMap<String, mpsc::Receiver<streamkit_core::types::Packet>>,
    batch_size: usize,
    asset_root: PathBuf,
) -> (NodeContext, MockOutputSender, mpsc::Receiver<NodeStateUpdate>) {
    let (mut ctx, sender, rx) = create_test_context(inputs, batch_size);
    ctx.asset_root = asset_root;
    (ctx, sender, rx)
}

#[allow(clippy::implicit_hasher)]
pub fn create_test_context_with_pin_mgmt(
    inputs: HashMap<String, mpsc::Receiver<streamkit_core::types::Packet>>,
    batch_size: usize,
) -> (
    NodeContext,
    MockOutputSender,
    mpsc::Receiver<NodeStateUpdate>,
    mpsc::Sender<streamkit_core::pins::PinManagementMessage>,
) {
    let (_control_tx, control_rx) = mpsc::channel(10);
    let (state_tx, state_rx) = mpsc::channel(10);
    let state_tx = streamkit_core::state::NodeStateSender::new(state_tx, 0);
    let (stats_tx, _stats_rx) = mpsc::channel(10);
    let (pin_mgmt_tx, pin_mgmt_rx) = mpsc::channel(10);

    let mock_sender = MockOutputSender::new();
    let output_sender = mock_sender.to_output_sender("test_node".to_string());

    let context = NodeContext {
        inputs,
        input_types: HashMap::new(),
        control_rx,
        output_sender,
        batch_size,
        state_tx,
        stats_tx: Some(stats_tx),
        telemetry_tx: None,
        session_id: None,
        cancellation_token: None,
        pin_management_rx: Some(pin_mgmt_rx),
        audio_pool: None,
        video_pool: None,
        pipeline_mode: streamkit_core::node::PipelineMode::Dynamic,
        view_data_tx: None,
        engine_control_tx: None,
        asset_root: test_asset_root(),
    };

    (context, mock_sender, state_rx, pin_mgmt_tx)
}

#[allow(clippy::implicit_hasher)]
pub fn create_oneshot_test_context(
    inputs: HashMap<String, mpsc::Receiver<streamkit_core::types::Packet>>,
    batch_size: usize,
) -> (NodeContext, MockOutputSender, mpsc::Receiver<NodeStateUpdate>) {
    let (mut context, mock_sender, state_rx) = create_test_context(inputs, batch_size);
    context.cancellation_token = Some(tokio_util::sync::CancellationToken::new());
    context.pipeline_mode = streamkit_core::node::PipelineMode::Oneshot;
    (context, mock_sender, state_rx)
}

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

    pub fn to_output_sender(&self, node_name: String) -> OutputSender {
        OutputSender::new(node_name, OutputRouting::Routed(self.sender.clone()))
    }

    pub async fn try_recv(&self) -> Option<(String, String, Packet)> {
        let mut receiver = self.receiver.lock().await;
        receiver
            .try_recv()
            .ok()
            .map(|(node, pin, packet)| (node.to_string(), pin.to_string(), packet))
    }

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

    pub async fn collect_packets(&self) -> Vec<(String, String, Packet)> {
        let mut packets = Vec::new();
        while let Some(packet) = self.try_recv().await {
            packets.push(packet);
        }
        packets
    }

    pub async fn get_packets_for_pin(&self, pin_name: &str) -> Vec<streamkit_core::types::Packet> {
        let all_packets = self.collect_packets().await;
        all_packets
            .into_iter()
            .filter(|(_, pin, _)| pin == pin_name)
            .map(|(_, _, packet)| packet)
            .collect()
    }
}

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

pub fn create_test_binary_packet(data: Vec<u8>) -> Packet {
    Packet::Binary { data: bytes::Bytes::from(data), content_type: None, metadata: None }
}

/// # Panics
/// Panics if the width/height/format combination is invalid.
#[allow(clippy::expect_used)]
pub fn create_test_video_frame(
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    fill_value: u8,
) -> VideoFrame {
    let layout = VideoLayout::packed(width, height, pixel_format);
    let mut data = vec![fill_value; layout.total_bytes()];

    if pixel_format == PixelFormat::I420 || pixel_format == PixelFormat::Nv12 {
        // Neutral chroma for predictable decoder output.
        // Works for both I420 (separate U/V planes) and NV12 (interleaved UV plane):
        // filling with 128 produces neutral grey regardless of interleaving.
        for plane in layout.planes().iter().skip(1) {
            let start = plane.offset;
            let end = start + plane.stride * plane.height as usize;
            data[start..end].fill(128);
        }
    }

    VideoFrame::new(width, height, pixel_format, data)
        .expect("test video frame dimensions/format should be valid")
}

pub fn create_test_video_packet(
    width: u32,
    height: u32,
    pixel_format: PixelFormat,
    fill_value: u8,
) -> Packet {
    Packet::Video(create_test_video_frame(width, height, pixel_format, fill_value))
}

/// Build an I420 [`VideoFrame`] with a deterministic, animated texture in the
/// luma plane (neutral chroma).
///
/// Unlike [`create_test_video_frame`]'s flat fill — which video encoders
/// compress down to a handful of bytes — this pattern carries enough spatial
/// detail to produce a realistically sized bitstream, and `shift` animates it
/// across frames. Useful for tests that need encoded output large enough to
/// exercise chunking/segmentation paths.
///
/// # Panics
/// Panics if the width/height are invalid for an I420 frame.
#[allow(clippy::expect_used)]
pub fn create_textured_video_frame(width: u32, height: u32, shift: u8) -> VideoFrame {
    let layout = VideoLayout::packed(width, height, PixelFormat::I420);
    let mut data = vec![128u8; layout.total_bytes()];

    let planes = layout.planes();
    let y_plane = &planes[0];
    let w = width as usize;
    let h = height as usize;
    let xb: Vec<u8> =
        (0..w).map(|x| u8::try_from(x.wrapping_mul(3) % 256).expect("masked to a byte")).collect();
    for row in 0..h {
        let yb = u8::try_from(row.wrapping_mul(5) % 256).expect("masked to a byte");
        let start = y_plane.offset + row * y_plane.stride;
        for col in 0..w {
            data[start + col] = (xb[col] ^ yb).wrapping_add(shift);
        }
    }

    VideoFrame::new(width, height, PixelFormat::I420, data)
        .expect("textured test video frame dimensions should be valid")
}

pub fn extract_audio_data(packet: &Packet) -> Option<&[f32]> {
    match packet {
        Packet::Audio(frame) => Some(&frame.samples),
        _ => None,
    }
}

/// # Panics
/// Panics if the state update is not received or doesn't match.
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

pub async fn assert_state_initializing(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(
        state_rx,
        |s| matches!(s, streamkit_core::NodeState::Initializing),
        "Initializing",
    )
    .await;
}

pub async fn assert_state_running(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(state_rx, |s| matches!(s, streamkit_core::NodeState::Running), "Running")
        .await;
}

pub async fn assert_state_stopped(state_rx: &mut mpsc::Receiver<NodeStateUpdate>) {
    assert_state_update(
        state_rx,
        |s| matches!(s, streamkit_core::NodeState::Stopped { .. }),
        "Stopped",
    )
    .await;
}

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
